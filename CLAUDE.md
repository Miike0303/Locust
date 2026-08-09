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
- KiriKiri/KAG is Experimental (loose .ks + unencrypted XP3 + FE FE 0/1/2; patch.xp3 write; synthetic fixtures + **real extract E2E** Ochiru Hitozuma `patch2.xp3` ~34k + Taimanin Asagi Premium Box multi-xp3 (**~299k → ~95k → ~19.5k → ~18.6k** after tag/`\` + **script-key dedupe** + TJS/brace noise); **real inject E2E** temp Ochiru: `--direct` → `patch.xp3` 3/3; drops pure ellipsis/dot filler + **pure `[tag]…` lines** (KAG trailing `\` continuation + nested `[]` attrs) + **`//` TJS comments**, brace-only `{`/`}`/`]`**, `for`/`var`/engine `tf.`/`sf.` assignments; **multi-source dedupe** by normalized script key: prefer `patchN.xp3` > other XP3 > loose, tool dumps (`unencrypted/`) lowest; no cxdec)
- YU-RIS is Experimental (loose YSTB .ybn + YPF unpack/repack common versions; synthetic fixtures + real-game YSTB/Injuu Kangoku RE; discovery **skips VNTranslationTools/output/gameupdate**, **dedupes yst*.ybn by basename** prefer `pac/`; drops control-char / short-ASCII crumbs + **engine script tokens** (`es.*`/`MAC.*`, digit asset codes `st01`/`HSE_056`, dotted cmds `BTN.PLATE`, snake/ALLCAPS ids, short lowercase resources, hotkeys); Injuu extract **~578k → ~128k → ~22.5k** after filters)
- QSP is Experimental (synthetic fixtures only; no real game tested yet)
- Length-aware binary slots: first-pass **HARD MAX** (budget ≤12, quotes source; ASCII utf8 notes accent cost); up to **2** length-aware retries (quote previous + exact excess); then **mechanical fit** (Latin accent-fold → despace → **multi-word first/initials** → **inner-vowel compress** → prefer longest non-truncated fit → encoding-aware truncate last) before giving up (`Opciones`→`Opcns` on 7-byte utf8); inject still skips any remaining oversize. **Glossary**: exact full-string hits short-circuit provider (`provider_used=glossary`) when in-budget; per-entry hints only terms present in source; **binary-slot hints drop glossary translations that exceed the byte budget**. **Real-provider ES E2E (grok-sub)** on Unity fixture `tmp/unity-length-e2e`: **0 oversize** after dual retry (`New Game`→`Nueva Pt`, `Load Game`→`Cargar J`, `Options`→`Opcns`). Mock dual-slot unit tests remain the regression gate
- Commercial Wolf RPG title E2E (no Wolf game on disk yet)
- Streaming verify/apply multi-GB: shipped (pack ZIP64 + stream apply); keep eye on edge cases
- Unity SerializedFile slice 2: type-tree blobs **skipped**; TextAsset (49) + MonoBehaviour (**114 or negative** script-type class ids) sequential aligned strings + **`string[]`/`List<string>`** (i32 count 1–64 + elements) + **TextMesh (141) `m_Text`** + **GUIText (132) `m_Text`** (Behaviour base + `m_PixelOffset`) extract/inject (pad in place, length-prefix byte-identical, `binary_slot=utf8`); mono walk skips up to 16 implausible 4-byte words between strings; mono field filter drops assembly-qualified types (short `UnityEngine.Object, UnityEngine` **and full .NET AQN** with `Version=`/`PublicKeyToken=` / `, Elringus.` / `, UnityEditor` / TMPro — BOXMAN ~300 rows) + API tokens (`set_text`) + namespaces (`Naninovel.Commands`) + `Assembly-CSharp` + Selectable ColorBlock states (`Highlighted`/`Pressed`/…) + engine tokens (`Naninovel`/`Default`) + slash paths (`Master/HFX`) + script cmds (`Gosub`/`Goto`/`Else`) + **Naninovel `@cmd` scenario blobs** (line/block starting with `@`) + **Lorem ipsum** placeholders + Live2D face params (`EyeR Open`/`Eyeball Y`/…) + `{var}` templates; drops **hex guid crumbs** (`72010b7a`) + component tokens (`Fader`/`Clip`/`Canvas`/`Sprites`/`trigger`/`Author Name`); keeps UI like `Q.SAVE`/`Play`/`Wait`/`Emily`; **TextAsset** skips binary + **line-break charset tables** (letter ratio under 12%, or no Latin word + ≤3 whitespace — catches CJK kana class tables); **ManagedText / locale docs** (`Key: Value` ≥70% of multi-line, or single dotted-key line) split into per-line entries (`textasset_loc_line`, value-only source + `loc_key`; inject rebuilds + pads whole `m_Script`); **skips BCP-47 locale-name catalogs** (`af`/`en-US`/`zh-Hans` keys — Naninovel `Locales`, BOXMAN ~233 rows); **skips non-player TextAsset names** (LineBreaking*/TECHNICAL SPECIFICATIONS*/CharacterNames) + internal markdown tech docs + **lorem loc values**; **simple CSV** (header identifiers, ≥2 data rows, no quotes) splits non-numeric columns into `textasset_csv_cell` (inject re-parses original blob + applies cells). **structural + heuristic extract keep duplicate texts** (unique path_id / offset slots; no text de-dupe). Heuristic skip ranges include structural classes **+ MonoScript (115) + Shader (48)** (type-name / HLSL noise). Heuristic filter also drops camelCase/PascalCase identifiers, `_` tokens, Unity hierarchy `"Name (N)"` clones, `Shaders/`/`Skybox/`/`UI/` paths, **built-in shader families** (`Mobile/`/`FX/`/`Nature/`/`Particles/`/…), **`Mesh Renderer (Id :N)`** labels, **`Base Layer.` animator state paths**, **C0 control-char soup**, **`… Atlas Material`**, uGUI defaults (`Sliding Area`/`Viewport`/`Thumbnail`), **all-lowercase code tokens**, **`…_NN` animation clips**, **slash+digit path segments**, **`… Sprite/Mesh/…` asset suffixes**, `Light 2D`, editor `(Selected)` suffixes, `naninovel/` audio crumbs. Format **v17–v22** (v22 LargeFilesSupport; v19 type-tree **32-byte** nodes + string buffer skip; **big-endian** structural TextAsset). BOXMAN full extract (post noise + ManagedText/CSV + AQN/Naninovel-@/Lorem filters): **~3.5k** estimated strings (mono bulk + loc/csv cells; prior ~4k DB minus ~300 AQN + ~170 script/lorem). Still no full type-tree field walk / object-table rewrite
- Unity heuristic extract/inject: **big-endian length prefix** support (`metadata.length_endian=be|le`); prefers LE; rejects BE off-by-3 shadow of following LE lengths (NUL payload start guard)
- Unity SerializedFile discovery: extensionless `globalgamemanagers` / `resources` / `level*` under `*_Data` (not only `*.assets`); walk depth 3 for nested `Scenes/…/level*`
- Unreal multi-GB base pak E2E: done (Last Hope); optional more titles
- RM MZ POR `.jsono` + Iavra multi-pack extract: done (prefer `en`); inject Replace re-encodes `.jsono`; inject Add writes `lang_*_{lang}.jsono` packs
- RM MZ UI language registration: `locust register-lang <game> -l es --label Español` (Iavra + VisuMZ langs arrays + Map boot choices; `*.bak-locust`); **desktop/server**: `POST /api/register-lang` + Tauri `register_lang` + Inject modal “Register … in game UI” for rpgmaker-* formats (**optional menu label** when one lang selected, same as CLI `--label`)
- Apply patch from URL: `locust apply <game> --url https://…/patch.zip` + desktop/server `zip_url` (desktop validates http(s) only + mutually exclusive with local path; **Patch modal**: shared `resolvePatchSource`, disable Verify/Apply until valid source, inline URL errors, remember last path/URL; register-lang warns when no UI patterns matched; **Inject modal**: optional “after inject also register-lang” for rpgmaker-* (persisted); inject modes filtered by `supported_modes` — **Replace-only formats default to Direct** and hide Add)
