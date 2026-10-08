# Architecture

Map at the observed `main` checkpoint `fe9d7b0`, a documentation-only closeout of hosted-tested source `530d705`. Start with the [improvement ledger](IMPROVEMENT-GOAL.md), especially its latest **Done** entry and backlog; dated handoffs are historical evidence, not the current queue. Code is authoritative when this map disagrees; this snapshot is not a promise about later changes.

## What it is

A game-translation tool: **extract** strings from a game into a per-game SQLite project, **translate** them with a provider, **validate**, then **inject** back into the game or **pack** a patch ZIP that end users **apply**/**roll back**. Two products share one engine: the desktop app and the `locust` CLI (the CLI is also the apply-only tool for players, `.github/workflows/release.yml` `cli` job).

## Workspace

Six Cargo members (`Cargo.toml:2-9`), workspace package version **0.1.0** (`Cargo.toml:13`):

| Crate | Role | Entry |
|---|---|---|
| `crates/core` | models, SQLite, `FormatPlugin` trait, translation engine (retry + rate limit), validation, placeholders, patch pack/apply/rollback, injection transactions, fonts, export | `crates/core/src/lib.rs:1-19` |
| `crates/formats` | 14 format plugins | `default_registry()` `crates/formats/src/lib.rs:34-54` |
| `crates/providers` | translation providers | `default_registry(&AppConfig)` `crates/providers/src/lib.rs:109` |
| `crates/server` | Axum JSON + WS API, loopback by default | `create_router` `crates/server/src/lib.rs:489`, bind `:549-568` |
| `crates/cli` | `locust` binary; use `locust --help` for subcommands | enum `crates/cli/src/main.rs:68-351` |
| `apps/desktop/src-tauri` | Tauri 2 shell; runs the Axum server in-process on an OS-allocated loopback port | `apps/desktop/src-tauri/src/main.rs:25-57` |

Frontend: React 19 + Vite 6 + TS, react-router, react-query, zustand (`apps/desktop/package.json`). Routes `/`, `/editor`, `/review`, `/memory`, `/settings` (`apps/desktop/src/App.tsx:36-41`).

## How the parts talk

```
React UI ──(Tauri invoke, 26 cmds)──► src-tauri/commands.rs ─┐
    │                                                        ├─► locust-core ─► SQLite <game>.locust.db
    └──(HTTP /api/*, WS)──► Axum (same process in desktop) ──┘        │
                                                                     ├─► locust-formats (read/write game files)
locust CLI ─────────────────────────────────────────────────────────┴─► locust-providers (HTTP to MT APIs)
```

- **Dual transport.** `IS_TAURI` picks `tauriInvoke` or `fetch` per call (`apps/desktop/src/lib/api.ts:14`, `:368-652`). Many calls use HTTP even inside Tauri (patch, memory, inject status, backups: `api.ts:321-330`, `:668-673`, `:733-854`).
- **Port.** CLI `locust server` defaults to **7842** (`crates/cli/src/main.rs:347-350`). Tauri keeps an OS-allocated `127.0.0.1:0` listener bound and reports its port through `invoke("get_server_port")` (`apps/desktop/src-tauri/src/main.rs:25-57`, `apps/desktop/src/lib/api.ts:30-44`). Plain browser: `/api` via the Vite proxy in dev (UI port 1420, backend 7842; `apps/desktop/vite.config.ts:7-15`), `http://localhost:7842/api` in a production build (`api.ts:44`). WebSockets go straight to the backend port (`api.ts:869-880`).
- **HTTP API and local-client protection.** See `create_router` (`crates/server/src/lib.rs:489-547`) for routes rather than a fixed total. `/health` is at the root, everything else under `/api`. Two WebSockets: `/api/translate/ws/:job_id` (`:510`) and `/api/patch/ws/:job_id` (`:518`). CORS permits the supported Tauri origins and loopback HTTP origins with an explicit port (`:371-408`, `:460-470`); the outer Host/Origin guard rejects foreign clients before handlers or CORS preflights run (`:473-486`, `:541-545`). Methods/headers are unrestricted within that origin policy, not globally permissive CORS.
- **Concurrency.** Server admission uses a CAS guard per project (`crates/server/src/lib.rs:101-106`, `:172-177`). Game writes take a per-game, non-reentrant file lock (`crates/core/src/patch/lock.rs:96-105`, `:197-207`).

## Data

Patch ZIPs contain `locust-patch.json` (schema version 1): patch metadata and file hashes, plus an optional `game` block with lowercase store keys, uppercase DLsite codes, a game version, and up to three pristine file fingerprints (smallest original size first, then path). `locust patch` accepts `--rj`, repeatable `--store-id`, and `--game-version`; it detects DLsite codes from the nearest game-path component unless `--no-detect-id` is set. `--astro` includes the identity and the written ZIP's size/hash in its catalog stub. Existing manifests without `game` remain compatible with apply/verify.
CLI and desktop share core identity resolution and release-entry rendering/writing. The desktop's copyable publish command targets PowerShell with single-quoted literal paths (embedded apostrophes are doubled), not portable shell syntax; uploads remain in the separate Rule95 site publisher. `POST /api/patch/pack` accepts `rj_code`, `store_ids`, `game_version`, `detect_id` (default true), and `entry_path`, while `GET /api/patch/identity` detects a code from the path string without opening a project.

Per-game project DB at `<game-parent>/<game-name>.locust.db`, with a fallback under the config dir (`resolve_project_db_path`, see `CLAUDE.md` "Project database"). Opening an existing project merges into it and never wipes it. `init_schema` (`crates/core/src/database.rs:337`) sets WAL + foreign keys (`:361-363`) and creates the tables `project_metadata`, `textasset_originals`, `strings` (13 columns), `glossary`, `translation_memory`, `validation_issues`, `translation_runs`, `injected_files` (`:365-434`). Migrations are ad hoc (`PRAGMA table_info` + `ALTER`, `:446-484`) with no `user_version`. Global translation memory lives in `global_memory.db`, which uses the same schema (`:1938-1957`).

Config: `config_dir()/config.json`. `LOCUST_DATA_DIR` overrides the root (`crates/core/src/config.rs:180-208`). API keys live in `config.providers[id].api_key` and are never read from env (`crates/providers/src/lib.rs:18`, `:131`). Other env vars: `LOCUST_CONFIG` (`crates/cli/src/main.rs:44`), `LOCUST_BACKUP_ROOT` (`crates/cli/src/main.rs:498`).

## Formats (14)

Registered at `crates/formats/src/lib.rs:34-54`. Stable is the trait default (`crates/core/src/extraction.rs:66-68`), inherited by `rpgmaker-mv`, `rpgmaker-vxa` and `renpy`. The other 11 override it to Experimental: html, nscripter, kirikiri, qsp, tyrano, sugarcube, unity, unreal, vntextpatch, wolf, yuris (e.g. `crates/formats/src/unity.rs:2812-2815`). No registered plugin is ComingSoon. `wasm-plugins` remains non-default and inert in shipped builds (`crates/core/Cargo.toml:47-49`).

Unity SerializedFile supports **v17–v22** containers (`crates/formats/src/unity_serialized.rs:82-84`, `:407-415`). Structural readers and bounded type-tree layouts cover TextAsset, TextMesh, GUIText and identified MonoBehaviour display fields; unsupported schemas stay opaque (`:6-26`, `:488-489`, `:1581-1730`). Supported UI fields use complete in-file layouts or verified MonoScript properties-hash layouts (`:34-38`); this is not arbitrary full-tree serialization or general object-table rewriting. Validated TextAsset resizing is separate from fixed-size field injection.

## Providers

Always registered: google, mock, argos, lmstudio, ollama. Key-gated: deepl, openai, claude, deepseek, grok, gemini. `grok-sub` is registered when OAuth tokens exist. Any unknown config id with a `base_url` becomes an OpenAI-compatible provider (`crates/providers/src/lib.rs:109-231`). Retry and rate limiting are applied at one call site (`crates/core/src/translation.rs:405`, `:535`, `:659-699`). Mock output is never written to translation memory (`translation.rs:1777`).

## Frontend state

`projectStore` (open project), `editorStore` (filter/selection/single job/validation worklist), `queueStore` (batch queue, module-level `activeJobId`), `draftStore` (localStorage drafts, cross-tab sync) — `apps/desktop/src/stores/*`. Frontend single-job and queue starts are mutually exclusive through `canStartSingleJob`/`canStartQueue` (`apps/desktop/src/lib/translationJob.ts:8-19`), while their session tracking remains separate. UI i18n uses matching `en.ts`/`es.ts` catalogs: `es` is typed against `en`, and tests check key parity (`apps/desktop/src/lib/i18n/es.ts:4`, `apps/desktop/src/lib/i18n/i18n.test.ts`).

## Tests and CI

Observed [hosted run 37551095569](https://github.com/Miike0303/Locust/actions/runs/37551095569) at exact tested source **`530d705`**, subsequently delivered to `main` (see the ledger's latest **Done** entry):

| Job | Rust passed / failed / ignored | Frontend passed / failed / ignored |
|---|---|---|
| Ubuntu 22.04 | 1989 / 0 / 19 | 104 / 0 / 0 |
| Windows | 1999 / 0 / 19 | 104 / 0 / 0 |

Both jobs passed clean `npm ci`, frontend build/type-check, frontend unit tests, `cargo fmt --all --check`, strict workspace/all-target Clippy and `cargo test --workspace`, including the Tauri crate. Ignored tests were not run. The frontend runner is `tsx --test` (`apps/desktop/package.json:11`); this is not native Tauri UI or game-runtime coverage. The separate headless PO error check passed 3/3 against mocked responses, not a live backend.

**CI triggers:** only `v*` tag pushes and `workflow_dispatch`, not pushes to `main` or PRs (`.github/workflows/ci.yml:3-8`). Jobs run on Ubuntu 22.04 and `windows-latest` (`:18-19`, `:70-72`), each with Node 24 and Rust 1.94.0, frontend install/build/unit checks followed by fmt → strict Clippy → workspace tests (`:30-67`, `:75-110`). Workspace checks include Tauri; they require the frontend build first.

## State and continuation

The parent confirmed remote and local `main` at **`fe9d7b0`**, a documentation-only closeout with source/workflow unchanged from hosted-tested `530d705`. This records that checkpoint, not an evergreen clean-tree or verification claim. Older dirty/incomplete-checkout and unpublished-commit snapshots are not current state.

Use the [ledger](IMPROVEMENT-GOAL.md) for current backlog, completed evidence and remaining limits, including cross-repository work. No sibling checkout was inspected for this refresh. The authorized sequence is one bounded work unit → complete checks → publish the feature branch and pass manual CI → fast-forward `main` before the next unit.
