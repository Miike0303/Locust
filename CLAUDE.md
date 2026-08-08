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
- Length-aware binary slots: engine retries once on oversize + counters; **real-provider ES E2E done** (2026-08-08): Unity heuristic fixture `tmp/unity-length-e2e` (6 utf8 binary_slot strings) via `locust translate -p grok-sub -s en -t es` — 526 tokens, ~2.4s; 3 fit first attempt (`Hola Mundo`, `Presiona una tecla`, `¿Seguro?`); 3 still over after length-aware retry on tight UI budgets (`New Game` 8→`Nuevo Jgo` 9, `Load Game` 9→`Cargar Jgo` 10, `Options` 7→`Opciones` 8); `locust validate` → 3× `ExceedsBinarySlot`; inject skips oversize / writes fitting. Mock dual-slot unit tests remain the regression gate
- Commercial Wolf RPG title E2E (no Wolf game on disk yet)
- Streaming verify/apply multi-GB: shipped (pack ZIP64 + stream apply); keep eye on edge cases
- Unity SerializedFile slice 1 done (TextAsset); still no typetree/MonoBehaviour/full rewrite
- Unreal multi-GB base pak E2E: done (Last Hope); optional more titles
- RM MZ POR `.jsono` + Iavra multi-pack extract: done (prefer `en`); inject Replace re-encodes `.jsono`; inject Add writes `lang_*_{lang}.jsono` packs
- RM MZ UI language registration: `locust register-lang <game> -l es --label Español` (Iavra + VisuMZ langs arrays + Map boot choices; `*.bak-locust`)
- Apply patch from URL: `locust apply <game> --url https://…/patch.zip` + desktop/server `zip_url`
