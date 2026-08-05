# Project Locust

Universal open-source game translation tool built in Rust.

## Architecture

Cargo workspace with 6 crates:
- `crates/core` — error types, models, database (SQLite), extraction traits, translation engine, config, encoding, placeholders, validation, backup, glossary, font validation, export (PO/XLIFF), WASM plugins
- `crates/formats` — game format plugins: RPG Maker MV/MZ, VX Ace, Ren'Py, Wolf RPG, QSP, TyranoBuilder, KiriKiri, YU-RIS, NScripter
- `crates/providers` — translation providers: Mock, Argos, DeepL, OpenAI, Claude, Ollama + retry/rate limiting
- `crates/server` — Axum HTTP server with 25+ REST endpoints, WebSocket for progress
- `crates/cli` — clap CLI with extract/translate/inject/validate/export/import/server commands
- `apps/desktop/src-tauri` — Tauri desktop app (React + Vite + TypeScript frontend in apps/desktop/)

## Build

```bash
# Rust backend
export PATH="$PATH:/c/msys64/mingw64/bin:/c/Users/Mike/.cargo/bin"
cargo test --workspace
cargo build --release -p locust-cli

# Frontend
cd apps/desktop
npm run build
```

## Key Commands

```bash
locust extract <game_path>          # Auto-detect format and extract strings
locust translate <db> -p mock       # Translate with provider
locust inject <game> -P <db> -l es  # Inject translations
locust validate <db>                # Placeholders + binary slot length (Unity/Unreal/Wolf)
locust patch … / apply …            # Package + apply patch zip (see docs/VN_RPG_TRANSLATION.md)
locust server --port 7842           # Start web server
locust formats                      # List supported formats (+ stability)
locust providers                    # List translation providers
```

## Pending Work

- TyranoBuilder is Experimental (`data/scenario/*.ks` UTF-8 only; synthetic fixtures; no asar / data.exe)
- NScripter is Experimental (`0.txt` / `00.txt` / `nscript.dat` only; synthetic fixtures; no NSA / nscr_sec / nscript.___)
- KiriKiri/KAG is Experimental (loose .ks only; synthetic fixtures; no XP3 / mode-2 yet)
- YU-RIS is Experimental (loose YSTB .ybn only; synthetic fixtures; no YPF unpack yet)
- QSP is Experimental (synthetic fixtures only; no real game tested yet)
- Length-aware *real* ES for binary engines (validate flags oversize; mock is dual-slot safe)
- Commercial Wolf RPG title E2E; Unreal full multi-GB base pak (patch `_P.pak` proven)
- Deeper Unity/Unreal parsers beyond heuristics
