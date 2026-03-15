# Session Log

## 2026-03-15

### Prompt #1 — Workspace Setup
- Created Rust Cargo workspace with 6 crates: `core`, `formats`, `providers`, `server`, `cli`, `desktop`
- Configured workspace.package defaults (version, edition, authors, license, repository)
- Added 19 shared workspace dependencies (tokio, axum, serde, rusqlite, reqwest, etc.)
- Each crate has minimal Cargo.toml referencing workspace deps and a passing `test_placeholder`
- Created root files: `.gitignore`, `README.md`, `LICENSE` (MIT 2025), `CONTRIBUTING.md`
- Created `.github/workflows/ci.yml` with test (matrix), lint, and build-binaries jobs
- Installed Rust toolchain, MSYS2/GCC, and VS Build Tools as prerequisites
- Verified: `cargo check --workspace` — OK, `cargo test --workspace` — 6/6 passed

### Prompt #2 — Core Modules (error + models)
- Implemented `error.rs` with `LocustError` (15 variants via thiserror) and `Result<T>` alias
- Implemented `models.rs` with `StringEntry`, `StringStatus`, `OutputMode`, `TranslationRequest`, `TranslationResult`, `ValidationIssue`, `ValidationKind`, `ProgressEvent`
- `StringEntry` methods: `new`, `with_context`, `with_tags`, `with_char_limit`, `source_hash`, `is_translatable`, `translation_exceeds_limit`
- `StringStatus` implements `Display` and `FromStr` (snake_case roundtrip)
- Created stub modules: extraction, translation, project, database, glossary, config, encoding, placeholder, validation, backup
- Updated `lib.rs` to `pub mod` all 12 modules and re-export `LocustError` + `Result`
- Verified: `cargo test -p locust-core -- models` — 9/9 passed

### Prompt #3 — Database Module
- Implemented `database.rs` with `Database` struct using `Mutex<Connection>`
- Schema: 4 tables (strings, glossary, translation_memory, validation_issues) with WAL mode
- Structs: `EntryFilter`, `ProjectStats`, `GlossaryEntry`
- 17 methods: CRUD for entries, translation memory, glossary, validation issues, stats
- Async methods use `tokio::task::spawn_blocking`
- Verified: `cargo test -p locust-core -- database` — 14/14 passed

### Prompt #4 — Extraction Module
- Implemented `FormatPlugin` trait (detect, extract, inject, inject_add) with default methods
- `InjectionReport` struct tracking files_modified, strings_written, strings_skipped, warnings
- `FormatRegistry` for plugin registration, detection by extension, lookup by id, listing
- `PluginInfo` struct for serializable plugin metadata
- MockFormatPlugin and MockFormatPlugin2 for testing
- Verified: `cargo test -p locust-core -- extraction` — 10/10 passed

### Prompt #5 — Translation Module
- Implemented `TranslationProvider` trait (async_trait) with translate, estimate_cost, health_check
- `TranslationOptions` with defaults (ja→en, batch_size=40, cost_limit, glossary, memory)
- `TranslationManager` orchestrating batched translation with memory cache, glossary hints, cost limits, cancellation
- `ProviderRegistry` for provider management
- Implemented `Glossary` struct in glossary.rs (build_hint, add/get/delete entries)
- Added tokio-util dependency for CancellationToken
- Verified: `cargo test -p locust-core -- translation` — 9/9 passed

### Prompt #6 — Glossary Module
- Rewrote `glossary.rs` with `add`, `get_all`, `delete`, `build_hint(source_lang, target_lang)`
- `build_hint` formats up to 50 entries as "term → translation" with header
- Updated `translation.rs` to use new `build_hint` signature and format
- Verified: `cargo test -p locust-core -- glossary` — 7/7 passed

### Prompt #7 — Config Module
- Implemented `AppConfig` with providers, UI settings, recent projects, load/save JSON
- `ProviderConfig`, `UiConfig`, `RecentProject` structs with serde defaults
- Platform-specific `config_dir()` (Windows/macOS/Linux)
- `add_recent_project` with dedup and max 10
- Verified: `cargo test -p locust-core -- config` — 10/10 passed

### Prompt #8 — Encoding & Placeholder Modules
- Implemented `encoding.rs`: `EncodingDetector` with detect_and_decode, encode_to_original, read_file_auto, write_file_encoded
- Supports UTF-8, UTF-8-BOM, Shift-JIS, EUC-JP, CP1252, CP1251, GB2312, Big5
- Implemented `placeholder.rs`: `PlaceholderProcessor` with extract, restore, validate
- Handles RPG Maker codes, HTML tags, Python/Rust/C format strings, custom brackets
- Added encoding_rs and chardet dependencies
- Verified: `cargo test -p locust-core -- encoding` — 6/6 passed
- Verified: `cargo test -p locust-core -- placeholder` — 10/10 passed

### Prompt #9 — Backup & Validation Modules
- Implemented `backup.rs`: `BackupManager` with create, restore, list, delete, delete_old_backups
- Recursive file copy with walkdir, manifest.json per backup, sorted listing
- Implemented `validation.rs`: `Validator` with validate_entry, validate_all, validate_and_save
- Checks: EmptyTranslation, IdenticalToSource, ExceedsCharLimit, placeholder mismatches
- `ValidationReport` with counts by kind
- Verified: `cargo test -p locust-core -- backup` — 6/6 passed
- Verified: `cargo test -p locust-core -- validation` — 7/7 passed

### Prompt #10 — RPG Maker MV/MZ Format Plugin
- Implemented `crates/formats` with `RpgMakerMvPlugin` (FormatPlugin trait)
- Detection: data dir with Actors.json/System.json/Map001.json, MV vs MZ version detection
- Extraction: Actors, System (gameTitle, terms), Maps (code 401 dialogue, 102 choices), CommonEvents
- Injection Replace: modify JSON in-place preserving structure
- Injection Add: MZ Languages/{lang}.json or MV www/data/i18n/{lang}.json (Iavra format)
- Created fixture JSON files for Actors, System, Map001
- Stub modules: rpgmaker_vxa, renpy, wolf_rpg
- Verified: `cargo test -p locust-formats -- rpgmaker_mv` — 15/15 passed

### Prompt #11 — RPG Maker VX Ace Plugin (Ruby Marshal)
- Implemented Ruby Marshal parser/writer (MarshalValue enum) supporting nil, bool, int, string, symbol, array, hash, object, user-defined, IVAR wrapper
- RpgMakerVxaPlugin: detect Data/*.rvdata2, extract actor fields, map events, common events
- Inject Replace mode: parse, modify strings, serialize back to valid Marshal binary
- inject_add returns UnsupportedFormat (VXA doesn't support Add mode)
- Registered in default_registry()
- Verified: `cargo test -p locust-formats -- rpgmaker_vxa` — 8/8 passed

### Prompt #12 — Ren'Py Plugin
- Implemented `RenPyPlugin` for .rpy visual novel scripts
- Extraction: say statements, menu choices, define strings, _() i18n calls
- Inject Replace: in-place string replacement preserving formatting
- Inject Add: Ren'Py native game/tl/{lang}/ translation files
- Entry IDs use filename#line_number format
- Fixture files: script.rpy and gui.rpy
- Verified: `cargo test -p locust-formats -- renpy` — 11/11 passed

### Prompt #13 — Wolf RPG Plugin
- Implemented `WolfRpgPlugin` with heuristic Shift-JIS string extraction from .wolf binaries
- Detection: Data/ dir with .wolf files
- Inject Replace: binary patching with null-byte padding, errors on longer translations
- All extracted entries tagged with `extraction_method: "heuristic"` metadata
- Fixture built programmatically with embedded Shift-JIS strings
- Verified: `cargo test -p locust-formats -- wolf_rpg` — 8/8 passed

### Prompt #14 — Multi-Language Injection Pipeline
- Implemented `MultiLangInjector` in extraction.rs with Replace and Add mode injection
- Replace mode: backup → copy project per language (hardlinks for media on Unix) → inject
- Add mode: backup → sequential inject_add per language
- `MultiLangReport` with processed/failed languages, backup_id, per-language reports
- Implemented server with `/api/formats`, `/api/formats/:id/modes`, `/api/inject` endpoints
- `copy_dir_for_inject` with platform-aware hardlink support
- Verified: `cargo test -p locust-core -- extraction` — 18/18 passed
- Verified: `cargo test -p locust-server` — 1/1 passed

### Prompt #15 — Font Validation Module
- Implemented `FontValidator` with check_coverage, find_game_fonts, check_game_fonts
- `FontCoverageReport` with missing chars, coverage percent, full coverage flag
- `suggest_replacement_font` for Latin Extended, CJK, Cyrillic, Arabic, etc.
- Minimal TTF font builder for testing (ASCII 0x20-0x7E coverage)
- Added ttf-parser dependency, pub mod font_validation in lib.rs
- Verified: `cargo test -p locust-core -- font_validation` — 7/7 passed

### Prompt #16 — Translation Providers Crate
- Implemented `crates/providers` with MockProvider, ArgosProvider, DeepLProvider
- ArgosProvider: offline/free via local API, batch translation, health check with install hints
- DeepLProvider: API with auth key, uppercase lang codes, Pro/Free tier cost estimation
- `default_registry()` auto-registers providers based on AppConfig
- Stub modules: openai, claude, ollama
- Verified: `cargo test -p locust-providers -- argos` — 5/5 passed
- Verified: `cargo test -p locust-providers -- deepl` — 6/6 passed

### Prompt #17 — OpenAI & Claude Providers
- Implemented `OpenAiProvider`: chat completions API, JSON array response parsing, lenient parse for markdown-wrapped responses, token-based cost estimation
- Implemented `ClaudeProvider`: Anthropic messages API, x-api-key + anthropic-version headers, haiku pricing
- Both use shared system prompt with game context and glossary hints
- Verified: `cargo test -p locust-providers -- openai` — 7/7 passed
- Verified: `cargo test -p locust-providers -- claude` — 4/4 passed

### Prompt #18 — Ollama Provider + Retry & Rate Limiting
- Implemented `OllamaProvider`: local LLM via /api/chat, health check with model detection
- Implemented `retry.rs`: `with_retry` with exponential backoff for retryable errors (429, 503, 502, timeout)
- `RateLimiter` with requests-per-minute windowed throttling
- `is_retryable` checks for rate limit, server errors, timeouts, IO errors
- Updated `default_registry()` to register OpenAI, Claude, and Ollama
- Verified: `cargo test -p locust-providers` — 32/32 passed

### Prompt #19 — Global Memory + Export/Import (PO & XLIFF)
- Added `GlobalMemoryDb` newtype in database.rs for cross-project translation memory
- `memory_count()` method on Database and GlobalMemoryDb
- Implemented `export.rs`: export_po, import_po, export_xliff, import_xliff
- PO format: GNU gettext with proper header, context, references
- XLIFF 1.2: XML with trans-unit elements, parsed via quick-xml
- Added quick-xml dependency
- Verified: `cargo test -p locust-core -- export` — 6/6 passed
- Verified: `cargo test -p locust-core -- database` — 16/16 passed (incl. global memory)

### Prompt #20 — Complete Axum HTTP Server
- Full REST API with 25+ endpoints: health, formats, providers, project, strings, translate, inject, validate, glossary, export/import, config, memory, backups
- AppState with Arc-wrapped shared state, DashMap for active jobs
- CORS permissive layer, test server with ephemeral port binding
- API key redaction in config endpoint
- Project open with auto-detection, string CRUD, translation job management
- Verified: `cargo test -p locust-server` — 16/16 passed

### Prompt #21 — Full CLI with clap
- Implemented CLI with clap derive macros: extract, translate, inject, validate, providers, formats, glossary, export, import, server
- indicatif progress bars for translation, comfy-table for formatted output
- Integration tests with assert_cmd: help, version, formats, providers, extract errors, glossary
- Verified: `cargo test -p locust-cli` — 8/8 passed (1 unit + 7 integration)
- Verified: `cargo build --release -p locust-cli` — success

### Prompt #22 — WASM Plugin System
- Implemented `wasm_plugin.rs` with `WasmPlugin` struct backed by wasmtime
- Plugin interface: locust_plugin_metadata, locust_extract, locust_inject, locust_alloc, locust_free
- Host provides locust_log import for tracing
- `load_wasm_plugin` and `scan_plugin_dir` for dynamic plugin discovery
- Feature-gated behind `wasm-plugins` feature (optional wasmtime dependency)
- Example WASM plugin skeleton in docs/plugin-example/ (txt line extractor)
- Plugin development guide in docs/plugin-example/plugin.md
- Verified: `cargo test -p locust-core -- wasm` — 4/4 passed (1 ignored, requires wasm32-wasi)
- Verified: `cargo check -p locust-core --features wasm-plugins` — compiles

### Prompt #23 — Tauri Desktop App Scaffold (React + Vite + TypeScript)
- Scaffolded apps/desktop/ with React 19, Vite 6, TypeScript, Tailwind CSS v4
- API client (`src/lib/api.ts`) with typed functions for all 25+ server endpoints
- WebSocket client (`src/lib/ws.ts`) for real-time translation progress
- Layout with sidebar navigation (Home, Editor, Settings)
- Welcome page with open folder, recent projects, supported formats grid
- Editor and Settings placeholder pages
- Verified: `npm run build` (tsc + vite) — success, dist/ produced

### Prompt #24 — String Editor Page & Components
- Zustand stores: projectStore (project/stats) and editorStore (filter/selection/job)
- `StringTable`: TanStack Table v8 with sorting, inline editable translation cells, status badges, file/tag display
- `FilterBar`: status pill buttons, debounced search, clear filters, showing X of Y count
- `DetailPanel`: full source/translation view, status buttons, char limit warning, collapsible metadata
- `TranslationModal`: 2-step (configure → progress), provider selector, WS progress updates, cost tracking
- `Editor` page: top bar with stats, translate/inject/validate/export buttons, filter+table+detail layout
- Verified: tsc --noEmit + vite build — success

### Prompt #25 — Settings, Review Mode, DiffView, Search & Replace
- `Settings` page: Providers (per-provider config cards, test connection), Translation Defaults, Appearance (theme/font), Data (backups)
- `DiffView` component: side-by-side source/translation diff with character-level highlighting via diff-match-patch
- `Review` page: one-at-a-time review with keyboard shortcuts (A=approve, S=skip, P=previous, E=edit, Escape=exit), progress bar, diff toggle
- `SearchReplace` modal: batch search/replace across translations with preview
- Added /review route, updated App.tsx
- Verified: tsc --noEmit + vite build — success
