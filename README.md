# fate-respec

Respec a FATE / FATE: Traitor's Soul character: refunds all skill and stat
points so you can spend them again in the in-game respec menu.

## Quick start

1. Download `fate-respec_windows-x64.exe` from the
   [releases](https://github.com/krakow10/fate-respec-tool/releases) page.
2. Find your save (`.FFD`) in
   `AppData\Local\WildTangent\Fate\Persistent\SAVE\en-US\` and back it up.
3. Run:

   ```
   fate-respec_windows-x64.exe my_save.FFD my_save_respec.FFD
   ```

4. Replace the original save with `my_save_respec.FFD` and respec in-game.

The tool prints a summary of what it refunded. Pass `--help` for the
`--version` and `--stat-baseline` options; the defaults work for most
characters.

## Building from source

Install Rust from <https://rustup.rs>, then:

```
git clone https://github.com/krakow10/fate-respec-tool.git
cd fate-respec-tool
cargo build --release
```

The binary is at `target\release\fate-respec.exe` and is used exactly as
above.

*Code generated with the Qwen 3.8 27B model.*
