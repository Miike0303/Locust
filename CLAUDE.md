# Project Locust

Universal open-source game translation tool built in Rust.

## Architecture

Cargo workspace with 6 crates:
- `crates/core` — error types, models, database (SQLite), extraction traits, translation engine, config, encoding, placeholders, validation, backup, glossary, font validation, export (PO/XLIFF), WASM plugins
- `crates/formats` — game format plugins: RPG Maker MV/MZ, VX Ace, Ren'Py, Wolf RPG
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
locust server --port 7842           # Start web server
locust formats                      # List supported formats
locust providers                    # List translation providers
```

## Session Log

See `session/session.md` for full development history (26 prompts implemented).

## Pending Work

- HTML game format plugin
- Unity game format plugin
- Unreal Engine format plugin
- Full Tauri desktop build (needs Tauri CLI setup)
- WebSocket endpoint implementation for live progress
