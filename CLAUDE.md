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

- TyranoBuilder is Experimental (`data/scenario/*.ks` loose + `app.asar` + NW.js `package.nw` / `data.exe` appended-zip; UTF-8; synthetic fixtures)
- NScripter is Experimental (`0.txt` / `00.txt` / `nscr_sec.dat` rot5 / `nscript.dat` XOR 0x84; synthetic fixtures; no NSA / nscript.___)
- KiriKiri/KAG is Experimental (loose .ks + unencrypted XP3 + FE FE 0/1/2; patch.xp3 write; synthetic fixtures; no cxdec)
- YU-RIS is Experimental (loose YSTB .ybn + YPF unpack/repack common versions; synthetic fixtures + real-game YSTB validated)
- QSP is Experimental (synthetic fixtures only; no real game tested yet)
- Length-aware binary slots: engine retries once on oversize + counters; real-provider ES E2E still pending (mock is dual-slot safe)
- Commercial Wolf RPG title E2E (no Wolf game on disk yet)
- Streaming verify/apply for multi-GB patch zips (pack is ZIP64-ready and proven on an 8.4GB base pak; apply still buffers under the 64MiB/entry zipsec budget)
- Deeper Unity parsers beyond heuristics; Unreal multi-GB base pak + full pak index
