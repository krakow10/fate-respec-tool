//! `fate-save-editor` — refund all allocated skill points in a FATE `.FFD` save.
//!
//! Reads a character save file, sets all 15 skill values to 0, and adds the
//! refunded total to the character's unused skill point pool, so the player can
//! respec their skills in-game. Everything else in the file is copied verbatim
//! to the output file.
//!
//! The save file layout is documented in `FFD_FORMAT.md` at the repository root
//! (and implemented in `reference-js-code/`). Only two fields need to be
//! located, and both sit at fixed offsets relative to `level` (which is reached
//! through the `PLAYER` marker, version header, name, ancestor name, and a
//! fixed 118-byte skip). The variable-length spell section is never parsed, so
//! this tool works even if that part of the format changes.
//!
//! Usage:
//!   fate-save-editor <input.FFD> <output.FFD> [--version auto|fate|traitorsoul]

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

/// Offsets, relative to the start of `level`, of the fields this tool uses.
const OFFSET_LEVEL: usize = 0;
const OFFSET_EXPERIENCE: usize = 4;
const OFFSET_UNUSED_SKILL_POINTS: usize = 68;
const OFFSET_SKILLS: usize = 105;
const SKILL_STRIDE: usize = 4;

/// The 15 skills in file order, with human-readable display names.
const SKILLS: [&str; 15] = [
    "sword", "club", "hammer", "axe", "spear", "staff", "polearm", "bow",
    "critical strike", "spellcasting", "dual wield", "shield", "attack magic",
    "defense magic", "charm magic",
];

/// Plausibility bounds used to validate a parse (and to auto-detect the game
/// version). Deliberately wide: they only need to reject the garbage produced
/// by parsing with the wrong header size, not to enforce gameplay limits.
const NAME_LEN_MAX: u16 = 255;
const MIN_LEVEL: i32 = 1;
const MAX_LEVEL: i32 = 999;
const SKILL_MAX: i32 = 9999;

const USAGE: &str = "\
usage: fate-save-editor <input.FFD> <output.FFD> [--version auto|fate|traitorsoul]

Reads a FATE character save, resets all 15 skill values to 0, and refunds the
total to the unused skill point pool, so you can respec your skills in-game.
Everything else in the file is copied unchanged to the output file.

options:
  --version <v>  which game version the save is from:
                   auto         detect it by validating the parse (default)
                   fate         FATE (original)
                   traitorsoul  FATE: Traitor's Soul
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
    /// Absolute offset of the `level` field; base for all fixed offsets.
    level_offset: usize,
    name: String,
    level: i32,
}

/// What a respec changed, for the summary output.
struct RespecResult {
    refunded: i32,
    old_unused: i32,
    new_unused: i32,
    /// (display name, value before reset), in file order.
    skills: Vec<(String, i32)>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (input, output, version) = parse_args()?;

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

    let respec = respec_skills(&mut data, save.level_offset)?;
    fs::write(&output, &data).map_err(|e| format!("could not write '{output}': {e}"))?;

    print_summary(&save, &respec, &input, &output, version.is_none());
    Ok(())
}

/// Parses CLI arguments: two positional paths and an optional `--version`.
fn parse_args() -> Result<(String, String, Option<GameVersion>), String> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        process::exit(0);
    }

    let mut positionals: Vec<String> = Vec::new();
    let mut version: Option<GameVersion> = None;

    let mut rest = args.into_iter();
    while let Some(arg) = rest.next() {
        if arg == "--version" {
            let value = rest
                .next()
                .ok_or_else(|| "--version needs a value (auto | fate | traitorsoul)".to_string())?;
            version = parse_version(&value)?;
        } else if let Some(value) = arg.strip_prefix("--version=") {
            version = parse_version(value)?;
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

    Ok((input.clone(), output.clone(), version))
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

    // The last field we touch (the skills block) must fit in the file.
    let required_end = level_offset
        .checked_add(OFFSET_SKILLS + SKILLS.len() * SKILL_STRIDE)
        .ok_or_else(|| format!("offset overflow for {}", version.label()))?;
    if required_end > data.len() {
        return Err(format!(
            "file ends before the skills block for {}",
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

    let unused = read_i32_at(data, level_offset + OFFSET_UNUSED_SKILL_POINTS)?;
    if unused < 0 {
        return Err(format!(
            "unused skill points parsed as {} (expected >= 0) for {}",
            unused,
            version.label()
        ));
    }

    Ok(ParsedSave {
        version,
        level_offset,
        name,
        level,
    })
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

/// Zeros all skills and refunds their total to the unused skill point pool.
/// The buffer is only touched where values actually change.
fn respec_skills(data: &mut [u8], level_offset: usize) -> Result<RespecResult, String> {
    let mut refunded: i32 = 0;
    let mut skills = Vec::with_capacity(SKILLS.len());

    for (i, skill) in SKILLS.iter().enumerate() {
        let off = level_offset + OFFSET_SKILLS + i * SKILL_STRIDE;
        let value = read_i32_at(&*data, off)?;
        refunded = refunded
            .checked_add(value)
            .ok_or_else(|| "skill point total overflowed (corrupt file?)".to_string())?;
        skills.push((skill.to_string(), value));
    }

    let unused_off = level_offset + OFFSET_UNUSED_SKILL_POINTS;
    let old_unused = read_i32_at(&*data, unused_off)?;
    let new_unused = old_unused
        .checked_add(refunded)
        .ok_or_else(|| "unused skill points overflowed (corrupt file?)".to_string())?;

    if refunded > 0 {
        for (i, entry) in skills.iter().enumerate() {
            if entry.1 != 0 {
                write_i32_at(data, level_offset + OFFSET_SKILLS + i * SKILL_STRIDE, 0)?;
            }
        }
        write_i32_at(data, unused_off, new_unused)?;
    }

    Ok(RespecResult {
        refunded,
        old_unused,
        new_unused,
        skills,
    })
}

fn print_summary(
    save: &ParsedSave,
    respec: &RespecResult,
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

    let width = respec.skills.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    for (name, value) in &respec.skills {
        println!("{name:width$} {value:>10}");
    }

    println!();
    if respec.refunded > 0 {
        println!(
            "Respec complete: {} skill points refunded (unusedSkillPoints {} -> {})",
            respec.refunded, respec.old_unused, respec.new_unused
        );
    } else {
        println!("No skill points were allocated - nothing to respec.");
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
