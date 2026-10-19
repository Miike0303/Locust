# Locust project report (2026-10-19)

Full-state analysis of the repository at branch `docs/release-v0.2.0-blocker` (main = `eddbab8`, v0.2.0 source). It consolidates five read-only mappings (docs/ledgers, formats, core/providers/server/CLI, desktop frontend, CI/release/tooling) plus parent spot checks. The execution plan derived from it lives in [odd/tasks/day-plan-2026-10-19.md](../odd/tasks/day-plan-2026-10-19.md).

**Evidence level:** source and documents were read; no build, test, CI, native UI or game run was performed for this report. Counts marked *measured* were taken by the parent on this checkout; everything else cites the file that states it. Proposals are labelled as such and are not existing features.

## 1. Summary

| Area | State |
|---|---|
| Product | Alpha 0.2.0. Shared Rust engine for CLI, HTTP API and Tauri desktop: extract → SQLite project → translate → validate → inject → patch pack/apply/rollback. |
| Engines | 15 registered plugins (`crates/formats/src/lib.rs:38-60`): 3 Stable (RPG Maker MV/MZ, RPG Maker XP/VX/Ace, Ren'Py), 12 Experimental. |
| Providers | 13 built-in registrations plus arbitrary OpenAI-compatible endpoints (`crates/providers/src/lib.rs:111-250`). |
| Tests (*measured*, static attribute count) | Rust `#[test]`/`#[tokio::test]`: core 947, formats 1049, providers 105, server 199, cli 206, tauri 34 = **2540**; 26 `#[ignore]`. Frontend: **63** test files. Last recorded run: frontend 554/0/0 (release ledger L11). |
| Delivery | v0.2.0 source is on `main` and tagged. **No signed v0.2.0 release exists**; signing failed on owner credentials ([release ledger](../odd/tasks/release-v0.2.0.md)). |
| Biggest risks | Release blocked; CI runs only on tags/manual; no dependency audit gate; Tauri CSP `null`; six Rust files over 3000 lines; partial-extraction paths that can look complete. |

## 2. What is done

### 2.1 Engine and pipeline

- Project database merge on reopen (never deletes translations; stale-source reset; incomplete extractions preserve rows). `CLAUDE.md` "Project database".
- Direct, Replace and Add injection with recoverable transactions, pristine backups, shared backup payloads and restore preflight (`crates/core/src/backup.rs`, `crates/core/src/extraction.rs:141-344`).
- Patch ZIP pack/apply/rollback, ZIP64 streaming, URL apply, identity fields, Rule95 entry generation (`docs/IMPROVEMENT-GOAL.md`, `odd/tasks/desktop-publish.md`).
- Strict JA→EN→ES pivot wrapper for Ren'Py, MV/MZ, KiriKiri and HTML; bounded independent Ace artifact proof (`odd/tasks/ja-en-es-pipeline.md`, `odd/tasks/engine-pipeline-expansion.md`).
- Godot 4.0–4.3 PCK v2 Dialogic, Experimental (`odd/tasks/godot-dialogic-native.md`).
- Length-aware binary slots with retries, glossary short-circuit, exact translation-memory cache (`crates/core/src/translation.rs:1671-1710`).
- Font coverage check and font patches (cmap only; `crates/core/src/font_validation.rs:258-260`).

### 2.2 Engines (per plugin)

| Plugin | Stability | Main containers | Notable limits |
|---|---|---|---|
| rpgmaker-mv | Stable | JSON, POR `.jsono`, Iavra packs | Rejects encrypted wrappers |
| rpgmaker-vxa | Stable | Ruby Marshal XP/VX/Ace | No RGSS archive reader |
| renpy | Stable | `.rpy`, `.rpa` | Compiled-only `.rpyc` not established |
| godot-dialogic | Exp. | PCK v2 `.dtl` | No PCK v3/encrypted |
| wolf-rpg | Exp. | `.wolf` heuristic SJIS scan | Not schema-aware; no commercial proof |
| sugarcube | Exp. | HTML passages | Joined inline markup rejected |
| html-game | Exp. | HTML/HTM byte ranges | No dynamic DOM/JS |
| unity | Exp. | SerializedFile, UnityFS, loose VN | Bounded schemas only |
| unreal | Exp. | PAK, LocRes, IoStore v5/v7/v8 | No encrypted containers |
| qsp | Exp. | QSPGAME | No legacy/CP1251/`.qsps` |
| tyrano | Exp. | `.ks`, asar, nw, appended ZIP | UTF-8 only |
| kirikiri | Exp. | `.ks`, unencrypted XP3 | No cxdec/Hxv4/SCN/TJS |
| yuris | Exp. | YSTB, YPF | Spanish accents not representable in SJIS |
| nscripter | Exp. | `0.txt`, `nscr_sec.dat`, `nscript.dat` | No NSA/SAR, `nscript.___` |
| vntextpatch | Exp. | VNTextPatch JSON | Interchange bridge only |

### 2.3 Surfaces

- **Server:** 52 route paths with 56 method handlers (23 GET including 2 WebSocket upgrades, 27 POST, 2 PATCH, 4 DELETE; *measured* in `crates/server/src/lib.rs:488-540`); loopback bind, Host/Origin guard (`:455-480`); project exclusion and cooperative cancellation.
- **CLI:** 26 subcommands (`crates/cli/src/main.rs:79-391`).
- **Desktop:** routes Welcome, Editor, Review, Translation Memory, Settings, NotFound (`apps/desktop/src/App.tsx:38-45`) and about 15 modals (translate, inject, patch, validation, font patch, export/import, replace, pivot, resume, recovery, queue). EN/ES UI, light/dark theme, modal focus management, durable localStorage drafts (`apps/desktop/src/lib/draftPersistence.ts`), updater.

### 2.4 Delivered ledgers

Complete bounded outcomes: current documentation, desktop flow audit, desktop publish, direct resume, Godot/Dialogic, pinned frontend runner, PO import localization, shared UTF-16, Windows tag CI, task reconciliation, autonomous safe queue, engine pipeline expansion (V1/V2). See [odd/TASKS.md](../odd/TASKS.md).

## 3. What is open

### 3.1 Blocked on the owner

| ID | Blocker | Owner action |
|---|---|---|
| Release R3/R4 | "incorrect updater private key password: Missing comment in secret key" on all five desktop jobs | Restore `TAURI_SIGNING_PRIVATE_KEY` and its password matching the configured public key; then authorize the release run |
| REL-4 | Ren'Py Structural-tier Add needs native confirmation | Give the confirmation in the native UI, then agent verifies install/rollback/reapply |
| PIPE-4G | Two original speaker headers without `>` (`CommonEvents.json#218`, `#231`) | Decide the boundary rule for malformed source headers |
| Rule95 / landing | Cloudflare Pages, R2, domain | Owner accounts |

### 3.2 Blocked on environment or research

| ID | Blocker |
|---|---|
| UNI-S | Private window-station setup fails with error 183; containment gates never reached. Needs an equally protective design or an isolated Windows environment. |
| UNI-2/3, VN-N3 | Depend on UNI-S; complete accented Spanish for YU-RIS needs an engine-compatible representation. |
| RGSS generic admission | Needs alias/continuation/shrink/alternate-encoding evidence or hard exclusions (agent-doable, synthetic). |

### 3.3 Verified code defects and risks

Parent-verified on this checkout:

| # | Finding | Evidence |
|---|---|---|
| D1 | HTML extraction silently skips unreadable or non-UTF-8 files, so a partial extraction reports as complete | `crates/formats/src/html_game.rs:171` (`if let Ok(content)`) |
| D2 | RGSS directory extraction only logs Marshal parse failures and returns success | `crates/formats/src/rpgmaker_vxa.rs:1585-1588` |
| D3 | MV/MZ logs and skips Iavra pack failures (same class as D1/D2) | `crates/formats/src/rpgmaker_mv.rs:1915-1920` |
| D4 | Public `MarshalValue::serialize()` panics on unrepresentable graphs | `crates/formats/src/rpgmaker_vxa.rs:210-212` |
| D5 | Rate limiter and limiter registry use `lock().unwrap()` (poisoned-mutex panic) | `crates/core/src/translation.rs:947,1015` |
| D6 | Glossary control-token invariant uses `expect` in production | `crates/core/src/translation.rs:1634` (mapper-reported) |
| D7 | WASM host logging adds signed pointer and length before bounds checks (feature off by default) | `crates/core/src/wasm_plugin.rs:49-53` (mapper-reported) |
| D8 | Wolf extraction deduplicates equal text per file, which may drop distinct physical slots | `crates/formats/src/wolf_rpg.rs:20-40` (needs reproduction; may be intentional) |
| D9 | `tools/verify-worktree.sh` prints `DONE` regardless of failed checks | `tools/verify-worktree.sh` (mapper-reported) |
| D10 | `tools/cleanup-scratch.ps1` runs `git worktree prune` even in preview mode | `tools/cleanup-scratch.ps1` (mapper-reported) |

Correction from the mappings: a reported YU-RIS "ignored decode error" is in a `#[cfg(test)]` helper (`crates/formats/src/yuris.rs:400-404`) and is **not** a production defect.

Known and accepted: case-insensitive replace misses Kelvin-sign/dotted-I through the SQL prefilter (`docs/IMPROVEMENT-GOAL.md:349`). 13 npm vulnerabilities recorded without repair (`odd/tasks/windows-tag-ci.md:39`).

### 3.4 CI, release and supply chain

- `ci.yml` triggers only on `v*` tags and manual dispatch (*measured*, `.github/workflows/ci.yml:4-8`). Branches and PRs get no automatic checks.
- Release does not depend on CI success for the tagged SHA; manifest runs even after desktop failures.
- Toolchain drift: CI Rust 1.94/Node 24, release stable Rust/Node 20; cargo without `--locked`.
- No `cargo audit`/`cargo deny`/`npm audit` gate, no SBOM or checksums, no macOS test job, `wasm-plugins` feature never built in CI.
- Tauri `"csp": null` (*measured*, `apps/desktop/src-tauri/tauri.conf.json:24`).
- No OS code signing or notarization (`RELEASE.md:9-19`); only updater signing.

### 3.5 Maintainability (*measured* line counts)

| File | Lines |
|---|---:|
| `crates/core/src/translation.rs` | 7392 |
| `crates/core/src/database.rs` | 6924 |
| `crates/server/src/lib.rs` | 6688 |
| `crates/core/src/extraction.rs` | 5039 |
| `crates/cli/src/main.rs` | 4832 |
| `crates/core/src/backup.rs` | 3137 |
| `apps/desktop/src/components/PatchModal.tsx` | 1389 |
| `apps/desktop/src/pages/Settings.tsx` | 1169 |
| `apps/desktop/src/components/InjectModal.tsx` | 901 |
| `apps/desktop/src/pages/Welcome.tsx` | 833 |

Counts include inline tests. Translation-settings form logic is repeated between `TranslationModal.tsx:109-138` and `QueuePanel.tsx:129-208`.

### 3.6 Documentation drift

- `odd/TASKS.md` says 14 ledgers with ten complete; `odd/tasks/` now holds 18 files (one is a Git-excluded private operation ledger).
- `docs/ARCHITECTURE.md` still states version 0.1.0 and old checkpoints.
- `CLAUDE.md` "Latest continuation (2026-09-17)" says drafts are memory-only; current code persists drafts in localStorage (`apps/desktop/src/lib/draftPersistence.ts`).
- `docs/IMPROVEMENT-GOAL.md:50` still calls Godot unavailable; superseded by GD-0..6.
- `odd/tasks/ja-en-es-pipeline.md:21` still calls F2 deferred; superseded.
- README lacks provider credential/privacy/cost guidance, unsigned-installer troubleshooting, config/log locations and post-injection QA guidance.

## 4. Proposed new systems, views and mechanics

Proposals only. Each one needs a short design pass before code.

| ID | Proposal | Builds on | Value |
|---|---|---|---|
| P1 | **Project dashboard** view: progress, cost, extraction warnings, validation summary, recovery state, next recommended action | `/stats`, `/stats/files`, `/runs`, `/validate`, `/inject/status` | One place to see where a project stands |
| P2 | **QA dashboard**: persistent issue categories, filters, resolution status and trend | Existing `validation_issues` table (`crates/core/src/database.rs:686-755`) | Turns the validation modal into a worklist |
| P3 | **Durable job queue**: persist the queue, retry failed items, job history | `stores/queueStore.ts` (in-memory today) | Long batches survive restarts |
| P4 | **Fuzzy translation memory**: similarity suggestions in the detail panel, TMX import/export | Exact TM cache (`translation.rs:1671-1705`), TM browser page | Consistency and fewer provider calls |
| P5 | **Character/speaker workspace**: group lines by speaker, per-character tone notes and glossary passed as provider context | Speaker-control preservation (`translation.rs:1610-1667`), `models.rs:416` context | Consistent voices across dialogue |
| P6 | **Per-string history**: revision table, author/provider attribution, source-change diff | `strings`, `translation_runs` tables; DiffView | Audit trail and undo |
| P7 | **Model-aware budgets**: per-model context windows and configurable pricing | Batching defaults `translation.rs:80-104`, cost checks `:1125-1153` | Accurate cost limits |
| P8 | **Text-fit preview**: message-box width simulation from font advance widths for RPG Maker/Ren'Py | `font_validation.rs` | Catch overflow before injection |
| P9 | **Semantic QA pass**: optional LLM review scoring with flagged rows | Provider layer | Quality signal without a full human pass |
| P10 | **Split lifecycle dialogs** into stepped flows; dedicated Backups/Recovery page | PatchModal, InjectModal, Settings → Data | Lower cognitive load |

## 5. Engine coverage gaps

Not supported natively (gap list, not a difficulty estimate): RPG Maker 2000/2003, Wolf encrypted/structural variants, KiriKiri cxdec/Hxv4/SCN/TJS, Unity IL2CPP and Addressables, Ren'Py compiled-only RPYC, Godot PCK v3 and encrypted PCK, GameMaker, Construct, Siglus, Artemis, BGI/Ethornell, CatSystem2, Malie, LiveMaker, Unreal encrypted or package-object localization.

Suggested order for research spikes: Ren'Py RPYC-only (high demand, Stable engine), Godot PCK v3 (natural extension), RPG Maker 2000/2003 (large audience), then the VN engines.

## 6. Plan

The session-by-session plan, acceptance checks and ordering are in [odd/tasks/day-plan-2026-10-19.md](../odd/tasks/day-plan-2026-10-19.md).
