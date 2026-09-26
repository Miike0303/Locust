# Architecture

Map of Project Locust as of 2026-09-25 (branch `feat/desktop-ux-kimi-k3-p2`, working tree — see "State" below). Every claim cites code; if code and this file disagree, the code wins and this file is stale.

## What it is

A game-translation tool: **extract** strings from a game into a per-game SQLite project, **translate** them with a provider, **validate**, then **inject** back into the game or **pack** a patch ZIP that end users **apply**/**roll back**. Two products share one engine: the desktop app and the `locust` CLI (the CLI is also the apply-only tool for players, `.github/workflows/release.yml` `cli` job).

## Workspace

Six Cargo members (`Cargo.toml:2-9`):

| Crate | Role | Entry |
|---|---|---|
| `crates/core` | models, SQLite, `FormatPlugin` trait, translation engine (retry + rate limit), validation, placeholders, patch pack/apply/rollback, injection transactions, fonts, export | `crates/core/src/lib.rs:1-19` |
| `crates/formats` | 14 format plugins | `default_registry()` `crates/formats/src/lib.rs:33` |
| `crates/providers` | translation providers | `default_registry(&AppConfig)` `crates/providers/src/lib.rs:109` |
| `crates/server` | Axum JSON + WS API, loopback only | `create_router` `crates/server/src/lib.rs:369`, bind `:428` |
| `crates/cli` | `locust` binary, 23 subcommands | enum `crates/cli/src/main.rs:49-278` |
| `apps/desktop/src-tauri` | Tauri 2 shell; runs the Axum server in-process on `127.0.0.1:0` and exposes 26 commands | `apps/desktop/src-tauri/src/main.rs:27-86` |

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
- **Port.** Tauri: `invoke("get_server_port")` (`api.ts:30-39`). Plain browser: `/api` via the Vite proxy in dev (`apps/desktop/vite.config.ts:8-15`, port 1420), hard-coded `http://localhost:7842/api` in a production build (`api.ts:41`). WebSockets always go straight to the port (`api.ts:859-869`).
- **HTTP API.** 48 route paths / 52 handlers (`crates/server/src/lib.rs:371-419`). `/health` is at the root, everything else under `/api`. Two WebSockets: `/api/translate/ws/:job_id` (`:389`) and `/api/patch/ws/:id` (`:397`). CORS is `permissive()` (`:420`).
- **Concurrency.** Server admission uses a CAS guard per project (`crates/server/src/lib.rs:101-106`, `:172-177`). Game writes take a per-game, non-reentrant file lock (`crates/core/src/patch/lock.rs:96-105`, `:197-207`).

## Data

Per-game project DB at `<game-parent>/<game-name>.locust.db`, with a fallback under the config dir (`resolve_project_db_path`, see `CLAUDE.md` "Project database"). Opening an existing project merges into it and never wipes it. `init_schema` (`crates/core/src/database.rs:337`) sets WAL + foreign keys (`:361-363`) and creates the tables `project_metadata`, `textasset_originals`, `strings` (13 columns), `glossary`, `translation_memory`, `validation_issues`, `translation_runs`, `injected_files` (`:365-434`). Migrations are ad hoc (`PRAGMA table_info` + `ALTER`, `:446-484`) with no `user_version`. Global translation memory lives in `global_memory.db`, which uses the same schema (`:1938-1957`).

Config: `config_dir()/config.json`. `LOCUST_DATA_DIR` overrides the root (`crates/core/src/config.rs:180-208`). API keys live in `config.providers[id].api_key` and are never read from env (`crates/providers/src/lib.rs:18`, `:131`). Other env vars: `LOCUST_CONFIG` (`crates/cli/src/main.rs:44`), `LOCUST_BACKUP_ROOT` (`crates/cli/src/main.rs:498`).

## Formats (14)

Registered at `crates/formats/src/lib.rs:35-52`. Stable is the trait default (`crates/core/src/extraction.rs:66-67`), inherited by `rpgmaker-mv`, `rpgmaker-vxa` and `renpy`. The other 11 override it to Experimental: html, nscripter, kirikiri, qsp, tyrano, sugarcube, unity, unreal, vntextpatch, wolf, yuris (e.g. `crates/formats/src/unity.rs:2465`). `wasm-plugins` is a non-default feature that nothing enables (`crates/core/Cargo.toml:48-49`).

## Providers

Always registered: google, mock, argos, lmstudio, ollama. Key-gated: deepl, openai, claude, deepseek, grok, gemini. `grok-sub` is registered when OAuth tokens exist. Any unknown config id with a `base_url` becomes an OpenAI-compatible provider (`crates/providers/src/lib.rs:109-231`). Retry and rate limiting are applied at one call site (`crates/core/src/translation.rs:405`, `:535`, `:659-699`). Mock output is never written to translation memory (`translation.rs:1777`).

## Frontend state

`projectStore` (open project), `editorStore` (filter/selection/single job/validation worklist), `queueStore` (batch queue, module-level `activeJobId`), `draftStore` (localStorage drafts, cross-tab sync) — `apps/desktop/src/stores/*`. Single-job and queue tracking are separate and do not exclude each other (`apps/desktop/src/lib/translationJobSession.ts:17-40`, `apps/desktop/src/stores/queueStore.ts:43-60`). UI i18n: `en.ts`/`es.ts`, 892 keys each; `es` is typed against `en`, so a missing key fails `tsc` (`apps/desktop/src/lib/i18n/es.ts:4`).

## Tests and CI

- **Rust:** 1453 pass / 17 ignored across 59 suites (run 2026-09-25). Tests with real-game paths are `#[ignore]` (`crates/formats/tests/real_games.rs:6`).
- **Frontend:** 31 `node:assert` files run by `tsx`, chained in `npm run test:unit`. There is no vitest or ESLint, and test files are excluded from `tsc` (`apps/desktop/tsconfig.json:21`).
- **CI:** Ubuntu only (`.github/workflows/ci.yml:18`), on push to `main` and on PRs. It runs build → unit → fmt → clippy → test.

## Sibling apps (`C:\Projects`)

| App | What | Tie to Locust |
|---|---|---|
| `rule95-patcher` (git, `main`, 1 commit `dc74ffd` + uncommitted `install.rs` +598) | Tauri 2 end-user app that installs Locust patch ZIPs in place or into a copy, with Undo. Vanilla JS UI (`ui/main.js`). 5 commands: `scan_games`, `dlsite_info`, `install_patch`, `verify_patch`, `rollback_patch` (`src/lib.rs:141-147`). Headless `apply_once` (`src/bin/apply_once.rs`). | Path dependency `../Locust/crates/core` (`Cargo.toml:27`). Uses `patch::{verify, apply, rollback, GameLock}`, `verify_with_lock`, `enforce_verify_gates` and `injection_transaction::*`, some of which exist only as untracked files in Locust, so a clean clone of both repos does not build. |
| `rule95` (**not a git repo**) | Static Astro 5 EN/ES patch catalog (`src/content.config.ts` schema, `src/data/patches/<lang>/*.md`). Target is Cloudflare Pages; MinIO for images (`deploy/`). | `locust patch --astro <path>` emits a frontmatter stub for it. The site does not link to or mention rule95-patcher. |

Verified pipeline (2026-09-25):
1. `locust extract`, `translate -p mock`, `inject --direct` and `patch --pristine` produce a strict-tier ZIP.
2. `apply_once --verify-only` reports `Clean`, apply succeeds, and the game's hashes then equal Locust's injected tree.
3. A re-verify reports `AlreadyApplied`, and `--rollback` restores the original hashes exactly.
4. A game the user modified is refused with a hash mismatch (exit 1).
5. Copy mode leaves the original untouched.

## State (read before trusting HEAD)

The last commit is `c71d7e7` (2026-09-03). Since then the working tree holds 97 modified files (+23145/−4234) and 98 untracked files. Tracked files declare modules that exist only as untracked files, for example `crates/core/src/lib.rs` → `injection_transaction`, `font_patch` and `textasset_group`, and `crates/server/src/lib.rs:3-4`. So HEAD plus only the tracked changes does not build. `tmp/` holds all QA evidence, and it is ignored only through `.git/info/exclude:19`.
