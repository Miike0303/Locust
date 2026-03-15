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
