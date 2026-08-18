# fate-respec

Resets skill points and main stats in a FATE / FATE: Traitor's Soul character
save (`.FFD` file), so you can respec without replaying the game. The tool
zeros all 15 skills, drops the four main stats (strength, dexterity, vitality,
magic) back to their starting baselines, and refunds the spent points to the
unused-point pools that the in-game respec menu reads. Everything else in the
save is copied unchanged, and the result is written to a **new** file — your
original save is never modified.

## Using the prebuilt binary

Prebuilt binaries are published in the
[GitHub releases](https://github.com/krakow10/fate-respec-tool/releases). The
latest release (v0.1.0) includes:

- `fate-respec_windows-x64.exe` — Windows
- `fate-respec_linux-x64` — Linux x64
- source archives, if you prefer to [build from source](#building-from-source)

1. **Back up your save files** before doing anything else.
2. Download the binary for your platform from the release page. (On Linux,
   make it executable first: `chmod +x fate-respec_linux-x64`.)
3. Find your save files. On Windows they live in
   `AppData/Local/WildTangent/Fate/Persistent/SAVE/en-US/` (one `.FFD` file
   per character; the `en-US` segment is the game's language — yours may
   differ). On other platforms, look for the equivalent WildTangent FATE save
   folder.
4. Run the tool, giving it the save to read and a new file to write:

   ```
   fate-respec_windows-x64.exe my_save.FFD my_save_respec.FFD
   ```

   (Linux: `./fate-respec_linux-x64 my_save.FFD my_save_respec.FFD`.)

   It prints your character's name, level, and detected game version, shows
   which skills and stats were reset, and reports how many points were
   refunded.
5. Replace the original save with the respec'd file and load the character
   in-game — the refunded points will be waiting in the respec menu.

### Options

| Option | What it does |
| --- | --- |
| `--version auto` (default) | Tries each known game version and uses the first one that parses cleanly. |
| `--version fate` / `--version traitorsoul` | Skip auto-detection and assume a specific game. |
| `--stat-baseline s,d,v,m` | Use custom stat baselines (order: strength, dexterity, vitality, magic, e.g. `25,20,20,10`) instead of the defaults (25, 20, 25, 10). Only needed if your character's ancestor starts with different stats. |
| `-h` / `--help` | Print usage help. |

Good to know:

- Input and output must be different paths — the tool refuses to overwrite
  your original save.
- The tool only ever takes points back, never grants new ones: stats are only
  lowered to their baseline.

## Building from source

If there is no prebuilt binary for your platform (or you simply prefer to
build it yourself), you only need the Rust toolchain — the project has no
dependencies.

1. Install Rust from <https://rustup.rs> (on Linux:
   `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).
2. Clone the repo and build it:

   ```
   git clone https://github.com/krakow10/fate-respec-tool.git
   cd fate-respec-tool
   cargo build --release
   ```

3. The binary is at `target/release/fate-respec`
   (`target/release/fate-respec.exe` on Windows) and is used exactly as the
   prebuilt one above.
