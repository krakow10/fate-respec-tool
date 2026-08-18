//! `fate-respec` — refund all allocated skill and stat points in a FATE `.FFD` save.
//!
//! Reads a character save file, sets all 15 skill values to 0, resets the four
//! main stats (strength, dexterity, vitality, magic) to their baselines, and
//! adds the refunded totals to the character's unused skill and stat point
//! pools, so the player can respec in-game. Everything else in the file is
//! copied verbatim to the output file.
//!
//! The save file layout is documented in `FFD_FORMAT.md` at the repository root
//! (and implemented in `reference-js-code/`). The skills sit at fixed offsets
//! relative to `level` (which is reached through the `PLAYER` marker, version
//! header, name, ancestor name, and a fixed 118-byte skip). The main stats sit
//! immediately after the variable-length spell section (18 length-prefixed
//! slots plus the active spell), so that section must be walked to find them;
//! the length and UTF-8 validation performed while walking it doubles as extra
//! defense for the game-version auto-detection.
//!
//! Usage:
//!   fate-respec <input.FFD> <output.FFD> [--version auto|fate|traitorsoul]
//!                     [--stat-baseline strength,dexterity,vitality,magic]

use std::env;
use std::fs;
use std::process;

/// ASCII marker that begins the character record somewhere in the file.
const MARKER: &[u8] = b"PLAYER";

/// Size of the fixed, unparsed header block after the marker, per game version.
const FATE_HEADER: usize = 107;
const TRAITORS_SOUL_HEADER: usize = 115;

/// Fixed gap between the end of the ancestor name and the `level` field.
const ANCESTOR_TO_LEVEL_SKIP: usize = 118;

/// Offsets, relative to the start of `level`, of the fixed-layout fields this tool uses.
const OFFSET_LEVEL: usize = 0;
const OFFSET_EXPERIENCE: usize = 4;
const OFFSET_UNUSED_STAT_POINTS: usize = 64;
const OFFSET_UNUSED_SKILL_POINTS: usize = 68;
const OFFSET_SKILLS: usize = 105;
const SKILL_STRIDE: usize = 4;
/// Where the variable-length spell section begins (immediately after the skills block).
const OFFSET_SPELL_START: usize = 165;

/// The 15 skills in file order, with human-readable display names.
const SKILLS: [&str; 15] = [
    "sword", "club", "hammer", "axe", "spear", "staff", "polearm", "bow",
    "critical strike", "spellcasting", "dual wield", "shield", "attack magic",
    "defense magic", "charm magic",
];

/// The 4 main stats in file order, with human-readable display names.
const STATS: [&str; 4] = ["strength", "dexterity", "vitality", "magic"];
/// Stride between main-stat slots: each int32 sits in an 8-byte slot
/// (4-byte value + 4 bytes of padding).
const STAT_STRIDE: usize = 8;

/// Baseline value of each main stat for a fresh level-1 character (verified
/// against a new-game save). If your character's ancestor has different
/// baselines, pass `--stat-baseline`.
const BASELINE_STATS: [i32; 4] = [25, 20, 25, 10];

/// Shape of the spell section: three lists (attack, defense, charm) of 6
/// slots each, followed by the active spell. Every entry is a u16 LE length
/// prefix plus UTF-8 bytes (an empty entry occupies only its 2-byte prefix).
const SPELL_LIST_NAMES: [&str; 3] = ["attack", "defense", "charm"];
const SPELL_SLOTS: usize = 6;

/// Offsets, relative to where the spell section ends, of the trailing record fields.
const OFFSET_GOLD: usize = 40;
const OFFSET_RECORD_END: usize = 44;

/// Plausibility bounds used to validate a parse (and to auto-detect the game
/// version). Deliberately wide: they only need to reject the garbage produced
/// by parsing with the wrong header size, not to enforce gameplay limits.
const NAME_LEN_MAX: u16 = 255;
const MIN_LEVEL: i32 = 1;
const MAX_LEVEL: i32 = 999;
const SKILL_MAX: i32 = 9999;
const STAT_MAX: i32 = 9999;

const USAGE: &str = "\
usage: fate-respec <input.FFD> <output.FFD>
       [--version auto|fate|traitorsoul]
       [--stat-baseline strength,dexterity,vitality,magic]

Reads a FATE character save, resets all 15 skill values to 0 and the four main
stats (strength, dexterity, vitality, magic) to their baselines, and refunds
the totals to the unused skill and stat point pools, so you can respec in-game.
Everything else in the file is copied unchanged to the output file.

options:
  --version <v>  which game version the save is from:
                   auto         detect it by validating the parse (default)
                   fate         FATE (original)
                   traitorsoul  FATE: Traitor's Soul
  --stat-baseline <s,d,v,m>
                 baselines for the four main stats (strength, dexterity,
                 vitality, magic), e.g. 25,20,25,10. Only needed if your
                 character's ancestor has different baselines than the
                 default (a fresh level-1 character).
  -h, --help     show this help
";

#[derive(Clone, Copy)]
enum GameVersion {
    Fate,
    TraitorsSoul,
}

impl GameVersion {
    fn header_size(self) -> usize {
        match self {
            GameVersion::Fate => FATE_HEADER,
            GameVersion::TraitorsSoul => TRAITORS_SOUL_HEADER,
        }
    }

    fn label(self) -> &'static str {
        match self {
            GameVersion::Fate => "FATE (original)",
            GameVersion::TraitorsSoul => "FATE: Traitor's Soul",
        }
    }
}

/// A save file that parsed and validated successfully.
struct ParsedSave {
    version: GameVersion,
    /// Absolute offset of the `level` field; base for the fixed offsets.
    level_offset: usize,
    /// Absolute offset where the spell section ends; base for the stat offsets.
    stats_offset: usize,
    name: String,
    level: i32,
}

/// What a respec changed, for the summary output.
struct RespecResult {
    refunded_skills: i32,
    refunded_stats: i32,
    old_unused_skills: i32,
    new_unused_skills: i32,
    old_unused_stats: i32,
    new_unused_stats: i32,
    /// (display name, value before reset), in file order.
    skills: Vec<(String, i32)>,
    /// (display name, value before reset), in file order.
    stats: Vec<(String, i32)>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (input, output, version, stat_baseline) = parse_args()?;

    if input == output {
        return Err(format!(
            "input and output are the same path ('{input}') - refusing to overwrite the original save"
        ));
    }

    let mut data = fs::read(&input).map_err(|e| format!("could not read '{input}': {e}"))?;

    let save = match version {
        Some(v) => parse(&data, v),
        None => parse_auto(&data),
    }?;

    let baselines = stat_baseline.unwrap_or(BASELINE_STATS);
    let respec = respec(&mut data, &save, baselines)?;
    fs::write(&output, &data).map_err(|e| format!("could not write '{output}': {e}"))?;

    print_summary(&save, &respec, baselines, &input, &output, version.is_none());
    Ok(())
}

/// Parses CLI arguments: two positional paths and the optional `--version`
/// and `--stat-baseline` options.
fn parse_args() -> Result<(String, String, Option<GameVersion>, Option<[i32; 4]>), String> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        process::exit(0);
    }

    let mut positionals: Vec<String> = Vec::new();
    let mut version: Option<GameVersion> = None;
    let mut stat_baseline: Option<[i32; 4]> = None;

    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        if arg == "--version" {
            let value = rest
                .next()
                .ok_or_else(|| "--version needs a value (auto | fate | traitorsoul)".to_string())?;
            version = parse_version(&value)?;
        } else if let Some(value) = arg.strip_prefix("--version=") {
            version = parse_version(value)?;
        } else if arg == "--stat-baseline" {
            let value = rest
                .next()
                .ok_or_else(|| "--stat-baseline needs a value (e.g. 25,20,25,10)".to_string())?;
            stat_baseline = Some(parse_stat_baseline(&value)?);
        } else if let Some(value) = arg.strip_prefix("--stat-baseline=") {
            stat_baseline = Some(parse_stat_baseline(value)?);
        } else if arg.starts_with('-') && arg != "-" {
            return Err(format!("unknown option '{arg}'\n\n{USAGE}"));
        } else {
            positionals.push(arg);
        }
    }

    let [input, output] = positionals.as_slice() else {
        return Err(format!(
            "expected two path arguments (input and output), found {}\n\n{USAGE}",
            positionals.len()
        ));
    };

    Ok((input.clone(), output.clone(), version, stat_baseline))
}

fn parse_version(value: &str) -> Result<Option<GameVersion>, String> {
    match value {
        "auto" => Ok(None),
        "fate" => Ok(Some(GameVersion::Fate)),
        "traitorsoul" => Ok(Some(GameVersion::TraitorsSoul)),
        other => Err(format!(
            "unknown game version '{other}' (expected: auto | fate | traitorsoul)"
        )),
    }
}

/// Parses `--stat-baseline` as four comma-separated integers, in the order
/// strength, dexterity, vitality, magic.
fn parse_stat_baseline(value: &str) -> Result<[i32; 4], String> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 4 {
        return Err(format!(
            "--stat-baseline expects 4 comma-separated values (strength,dexterity,vitality,magic), found {} in '{}'",
            parts.len(),
            value
        ));
    }

    let mut baselines = [0; 4];
    for (slot, part) in baselines.iter_mut().zip(&parts) {
        let trimmed = part.trim();
        let v: i32 = trimmed.parse().map_err(|_| {
            format!(
                "invalid stat baseline '{trimmed}' (expected four comma-separated integers, e.g. 25,20,25,10)"
            )
        })?;
        if v < 0 || v > STAT_MAX {
            return Err(format!("stat baseline {v} out of range 0..={STAT_MAX}"));
        }
        *slot = v;
    }
    Ok(baselines)
}

/// Locates and validates the fields this tool touches, assuming a specific
/// game version. On success the file is very likely a save from that version;
/// each check exists to reject the garbage produced by the *other* version's
/// header size.
fn parse(data: &[u8], version: GameVersion) -> Result<ParsedSave, String> {
    let marker = data
        .windows(MARKER.len())
        .position(|window| window == MARKER)
        .ok_or_else(|| "no \"PLAYER\" marker found - is this a FATE save file?".to_string())?;

    let mut offset = marker + MARKER.len() + version.header_size();

    // Player name: u16 LE length + UTF-8 bytes.
    let name_len = read_u16(data, &mut offset)?;
    if name_len > NAME_LEN_MAX {
        return Err(format!(
            "player name length {} is implausible for {}",
            name_len,
            version.label()
        ));
    }
    let name = std::str::from_utf8(read_bytes(data, &mut offset, name_len as usize)?)
        .map(|s| s.to_string())
        .map_err(|_| format!("player name is not valid UTF-8 for {}", version.label()))?;

    // Ancestor name: same shape as the player name, skipped.
    let ancestor_len = read_u16(data, &mut offset)?;
    if ancestor_len > NAME_LEN_MAX {
        return Err(format!(
            "ancestor name length {} is implausible for {}",
            ancestor_len,
            version.label()
        ));
    }
    read_bytes(data, &mut offset, ancestor_len as usize)?;

    // Fixed gap, then the level chunk.
    offset += ANCESTOR_TO_LEVEL_SKIP;
    let level_offset = offset;

    // The spell section starts where the skills block ends, so the file must
    // at least reach that point.
    let required_end = level_offset
        .checked_add(OFFSET_SPELL_START)
        .ok_or_else(|| format!("offset overflow for {}", version.label()))?;
    if required_end > data.len() {
        return Err(format!(
            "file ends before the spell section for {}",
            version.label()
        ));
    }

    let level = read_i32_at(data, level_offset + OFFSET_LEVEL)?;
    if level < MIN_LEVEL || level > MAX_LEVEL {
        return Err(format!(
            "level parsed as {} (expected {}..={}) for {}",
            level,
            MIN_LEVEL,
            MAX_LEVEL,
            version.label()
        ));
    }

    let experience = read_i32_at(data, level_offset + OFFSET_EXPERIENCE)?;
    if experience < 0 {
        return Err(format!(
            "experience parsed as {} (expected >= 0) for {}",
            experience,
            version.label()
        ));
    }

    for (i, skill) in SKILLS.iter().enumerate() {
        let value = read_i32_at(data, level_offset + OFFSET_SKILLS + i * SKILL_STRIDE)?;
        if value < 0 || value > SKILL_MAX {
            return Err(format!(
                "'{skill}' skill parsed as {} (expected 0..={}) for {}",
                value,
                SKILL_MAX,
                version.label()
            ));
        }
    }

    let unused_stat_points = read_i32_at(data, level_offset + OFFSET_UNUSED_STAT_POINTS)?;
    if unused_stat_points < 0 {
        return Err(format!(
            "unused stat points parsed as {} (expected >= 0) for {}",
            unused_stat_points,
            version.label()
        ));
    }

    let unused_skill_points = read_i32_at(data, level_offset + OFFSET_UNUSED_SKILL_POINTS)?;
    if unused_skill_points < 0 {
        return Err(format!(
            "unused skill points parsed as {} (expected >= 0) for {}",
            unused_skill_points,
            version.label()
        ));
    }

    // The main stats sit right after the variable-length spell section, so
    // walk it (18 slots + active spell) to find where the stats begin.
    let stats_offset = walk_spells(data, level_offset + OFFSET_SPELL_START, version)?;

    // The record ends at `gold` (spell end + 44) and must fit in the file.
    let required_end = stats_offset
        .checked_add(OFFSET_RECORD_END)
        .ok_or_else(|| format!("offset overflow for {}", version.label()))?;
    if required_end > data.len() {
        return Err(format!(
            "file ends before the end of the character record for {}",
            version.label()
        ));
    }

    for (i, stat) in STATS.iter().enumerate() {
        let value = read_i32_at(data, stats_offset + i * STAT_STRIDE)?;
        if value < 0 || value > STAT_MAX {
            return Err(format!(
                "'{stat}' stat parsed as {} (expected 0..={}) for {}",
                value,
                STAT_MAX,
                version.label()
            ));
        }
    }

    let gold = read_i32_at(data, stats_offset + OFFSET_GOLD)?;
    if gold < 0 {
        return Err(format!(
            "gold parsed as {} (expected >= 0) for {}",
            gold,
            version.label()
        ));
    }

    Ok(ParsedSave {
        version,
        level_offset,
        stats_offset,
        name,
        level,
    })
}

/// Walks the variable-length spell section (18 length-prefixed slots, then
/// the active spell) starting at `offset`, and returns the offset where it
/// ends - the base for the main-stat offsets. The length and UTF-8 validation
/// doubles as version detection: parsing with the wrong header size produces
/// garbage lengths or invalid UTF-8 here.
fn walk_spells(data: &[u8], offset: usize, version: GameVersion) -> Result<usize, String> {
    let mut offset = offset;
    for list in SPELL_LIST_NAMES {
        for slot in 1..=SPELL_SLOTS {
            read_spell_name(data, &mut offset, version, &format!("{list} spell slot {slot}"))?;
        }
    }
    read_spell_name(data, &mut offset, version, "active spell")?;
    Ok(offset)
}

/// Reads one length-prefixed spell name (`label` names it in error messages),
/// validating its length and UTF-8 encoding.
fn read_spell_name(
    data: &[u8],
    offset: &mut usize,
    version: GameVersion,
    label: &str,
) -> Result<(), String> {
    let len = read_u16(data, offset)?;
    if len > NAME_LEN_MAX {
        return Err(format!(
            "{label} length {len} is implausible for {}",
            version.label()
        ));
    }
    if len > 0 {
        std::str::from_utf8(read_bytes(data, offset, len as usize)?)
            .map_err(|_| format!("{label} is not valid UTF-8 for {}", version.label()))?;
    }
    Ok(())
}

/// Tries each known game version and returns the first whose parse validates.
fn parse_auto(data: &[u8]) -> Result<ParsedSave, String> {
    let mut failures = Vec::new();
    for version in [GameVersion::Fate, GameVersion::TraitorsSoul] {
        match parse(data, version) {
            Ok(save) => return Ok(save),
            Err(reason) => failures.push(format!("{}: {}", version.label(), reason)),
        }
    }
    Err(format!(
        "the file does not parse as any known save version:\n  - {}\n\n\
         If this is a modded or otherwise unusual save, try passing \
         --version fate or --version traitorsoul explicitly.",
        failures.join("\n  - ")
    ))
}

/// Zeros all skill values, resets any main stats above their baselines to
/// those baselines, and refunds the totals to the unused skill and stat point
/// pools. Stats at or below their baseline are left untouched (a respec never
/// *grants* points), and the buffer is only touched where values change.
fn respec(
    data: &mut [u8],
    save: &ParsedSave,
    baselines: [i32; 4],
) -> Result<RespecResult, String> {
    let level_offset = save.level_offset;
    let stats_offset = save.stats_offset;

    // Skills start at 0, so the refund is the full value.
    let mut refunded_skills: i32 = 0;
    let mut skills = Vec::with_capacity(SKILLS.len());
    for (i, skill) in SKILLS.iter().enumerate() {
        let value = read_i32_at(&*data, level_offset + OFFSET_SKILLS + i * SKILL_STRIDE)?;
        refunded_skills = refunded_skills
            .checked_add(value)
            .ok_or_else(|| "skill point total overflowed (corrupt file?)".to_string())?;
        skills.push((skill.to_string(), value));
    }

    // Main stats have non-zero baselines, so the refund is value - baseline
    // (floored at 0).
    let mut refunded_stats: i32 = 0;
    let mut stats = Vec::with_capacity(STATS.len());
    for (i, stat) in STATS.iter().enumerate() {
        let value = read_i32_at(&*data, stats_offset + i * STAT_STRIDE)?;
        let refund = value.saturating_sub(baselines[i]);
        refunded_stats = refunded_stats
            .checked_add(refund)
            .ok_or_else(|| "stat point total overflowed (corrupt file?)".to_string())?;
        stats.push((stat.to_string(), value));
    }

    let old_unused_skills = read_i32_at(&*data, level_offset + OFFSET_UNUSED_SKILL_POINTS)?;
    let new_unused_skills = old_unused_skills
        .checked_add(refunded_skills)
        .ok_or_else(|| "unused skill points overflowed (corrupt file?)".to_string())?;

    let old_unused_stats = read_i32_at(&*data, level_offset + OFFSET_UNUSED_STAT_POINTS)?;
    let new_unused_stats = old_unused_stats
        .checked_add(refunded_stats)
        .ok_or_else(|| "unused stat points overflowed (corrupt file?)".to_string())?;

    // Writes stay byte-minimal: each point pool is only written when it
    // actually changed, and each slot only where its value differs.
    if refunded_skills > 0 {
        for (i, entry) in skills.iter().enumerate() {
            if entry.1 != 0 {
                write_i32_at(data, level_offset + OFFSET_SKILLS + i * SKILL_STRIDE, 0)?;
            }
        }
        write_i32_at(data, level_offset + OFFSET_UNUSED_SKILL_POINTS, new_unused_skills)?;
    }

    if refunded_stats > 0 {
        for (i, entry) in stats.iter().enumerate() {
            if entry.1 > baselines[i] {
                write_i32_at(data, stats_offset + i * STAT_STRIDE, baselines[i])?;
            }
        }
        write_i32_at(data, level_offset + OFFSET_UNUSED_STAT_POINTS, new_unused_stats)?;
    }

    Ok(RespecResult {
        refunded_skills,
        refunded_stats,
        old_unused_skills,
        new_unused_skills,
        old_unused_stats,
        new_unused_stats,
        skills,
        stats,
    })
}

/// Prints what the respec did: the loaded character, per-skill and per-stat
/// tables (a row shows an arrow only when it was reset), and the combined
/// refund line.
fn print_summary(
    save: &ParsedSave,
    respec: &RespecResult,
    baselines: [i32; 4],
    input: &str,
    output: &str,
    detected: bool,
) {
    println!("Loaded '{input}'");
    println!("Character: {}", save.name);
    if detected {
        println!("Version:   {} (auto-detected)", save.version.label());
    } else {
        println!("Version:   {}", save.version.label());
    }
    println!("Level:     {}", save.level);
    println!();

    // Skills always reset to 0, so a row only changes when it was non-zero.
    let skill_width = respec.skills.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    println!("Skills:");
    for (_i, (name, value)) in respec.skills.iter().enumerate() {
        if *value > 0 {
            println!("{name:skill_width$} {value:>10} -> 0");
        } else {
            println!("{name:skill_width$} {value:>10}");
        }
    }
    println!();

    // A stat only resets when it is above its baseline (a respec never
    // *grants* points), so rows at or below the baseline show no arrow.
    let stat_width = respec.stats.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    println!("Stats:");
    for (i, (name, value)) in respec.stats.iter().enumerate() {
        let baseline = baselines[i];
        if *value > baseline {
            println!("{name:stat_width$} {value:>10} -> {baseline}");
        } else {
            println!("{name:stat_width$} {value:>10}");
        }
    }
    println!();

    if respec.refunded_skills > 0 && respec.refunded_stats > 0 {
        println!("Respec complete: {} skill points and {} stat points refunded (unusedSkillPoints {} -> {}, unusedStatPoints {} -> {})", respec.refunded_skills, respec.refunded_stats, respec.old_unused_skills, respec.new_unused_skills, respec.old_unused_stats, respec.new_unused_stats);
    } else if respec.refunded_skills > 0 {
        println!("Respec complete: {} skill points refunded (unusedSkillPoints {} -> {})", respec.refunded_skills, respec.old_unused_skills, respec.new_unused_skills);
    } else if respec.refunded_stats > 0 {
        println!("Respec complete: {} stat points refunded (unusedStatPoints {} -> {})", respec.refunded_stats, respec.old_unused_stats, respec.new_unused_stats);
    } else {
        println!("No skill or stat points were allocated - nothing to respec.");
    }
    println!("Wrote '{output}'");
}

/// Reads a u16 LE value at `*offset` and advances `*offset` past it.
fn read_u16(data: &[u8], offset: &mut usize) -> Result<u16, String> {
    let end = (*offset).checked_add(2).ok_or("byte offset overflow")?;
    let bytes = data
        .get(*offset..end)
        .ok_or_else(|| format!("file ends before offset {end}"))?;
    *offset = end;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Reads `len` bytes at `*offset` and advances `*offset` past them.
fn read_bytes<'a>(data: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], String> {
    let end = (*offset).checked_add(len).ok_or("byte offset overflow")?;
    let bytes = data
        .get(*offset..end)
        .ok_or_else(|| format!("file ends before offset {end}"))?;
    *offset = end;
    Ok(bytes)
}

/// Reads an i32 LE value at `offset`.
fn read_i32_at(data: &[u8], offset: usize) -> Result<i32, String> {
    let end = offset.checked_add(4).ok_or("byte offset overflow")?;
    let bytes = data
        .get(offset..end)
        .ok_or_else(|| format!("file ends before offset {end}"))?;
    Ok(i32::from_le_bytes(bytes.try_into().unwrap()))
}

/// Writes an i32 LE value at `offset`.
fn write_i32_at(data: &mut [u8], offset: usize, value: i32) -> Result<(), String> {
    let end = offset.checked_add(4).ok_or("byte offset overflow")?;
    let bytes = data
        .get_mut(offset..end)
        .ok_or_else(|| format!("file ends before offset {end}"))?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
