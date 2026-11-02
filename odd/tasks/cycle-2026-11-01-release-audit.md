# Cycle 2026-11-01 — release-readiness audit (RA-1)

One Codex gpt-6.1-sol high researcher, read-only, on feat after W9-ENC/W8-07. Parent spot-checked: `crates/server/src/lib.rs:372` (startup `expect`), `.github/workflows/release.yml:80` (`releaseDraft: false`), `odd/tasks/release-v0.2.0.md` L15-L17 (v0.2.0 release failed at signing; no public v0.2.0 release exists; owner must restore the updater private key + password secrets matching the configured public key).

Parent decisions: next version **0.3.0** (not 1.0). Do not tag or publish. Owner-only items: signing secrets, OS code signing/notarization accounts, domains/DNS/VPS. Never expose the Axum server publicly.

**H — Verdict: do not tag the production release yet. Prepare 0.3.0, subject to the gates below.** The repository records unresolved signing delivery, incomplete current integration verification, and startup panic paths. These prevent a defensible production-ready claim. (`odd/tasks/release-v0.2.0.md:29`, `odd/tasks/cycle-2026-11-01.md:77`, `crates/server/src/lib.rs:372`)

**F** means verified in the cited repository source; historical execution results are identified as recorded evidence. **H** means recommendation or inference. This audit made no changes and ran no builds, tests, or network calls. Acceptance checks below are proposed future work.

**1. Must-fix before tagging**

1. **Signing credentials and updater acceptance — S, owner dependency.**

   **F:** The last recorded v0.2.0 release attempt failed all five desktop signing steps with “incorrect updater private key password: Missing comment in secret key”; CLI publication was skipped. This establishes a historical credential failure, not the current contents or availability of GitHub secrets. (`odd/tasks/release-v0.2.0.md:29`)

   **F:** Secret names are wired consistently: `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Signed updater artifacts are enabled, and a public key is configured. There is no source evidence of a misnamed secret or disabled artifact production. (`.github/workflows/release.yml:71`, `apps/desktop/src-tauri/tauri.conf.json:29`, `apps/desktop/src-tauri/tauri.conf.json:62`)

   **H — Action/files:** Owner validates or repairs the existing repository secrets against the configured public key. Keep the signing identity intact. Record the outcome in `odd/tasks/release-v0.2.0.md` and the next release acceptance report.

   **H — Acceptance:** All expected installers and exact `.sig` companions exist; supported tooling verifies signatures against the configured key; an isolated older installation successfully checks, downloads, installs, and relaunches. Shape validation alone is insufficient. (`RELEASE.md:53`, `RELEASE.md:57`, `RELEASE.md:81`)

2. **Prevent an incomplete release from becoming public/latest — M.**

   **F:** Each desktop matrix job publishes with `releaseDraft: false`; CLI jobs wait for the entire desktop matrix. The manifest job runs even after desktop failures, and the generator intentionally accepts any nonempty subset of complete platform pairs. (`.github/workflows/release.yml:80`, `.github/workflows/release.yml:89`, `.github/workflows/release.yml:150`, `scripts/generate-updater-manifest.py:69`)

   **H:** A partially successful desktop matrix can therefore expose a public release with missing platforms and no CLI delivery. This is an inference from workflow ordering, not an observed new failure. Partial manifests are explicitly supported; they must not be mistaken for complete release acceptance. (`RELEASE.md:60`)

   **H — Edit:** `.github/workflows/release.yml`, `scripts/generate-updater-manifest.py`, `tools/test_updater_manifest.py`, `RELEASE.md`, `docs/update-server.md`.

   **H — Acceptance:** Build into a draft; verify the declared platform inventory, CLI inventory, signatures, manifest URLs, and candidate SHA; publish only from one finalizer. Preserve partial-manifest generation as a recovery capability. Reject non-version manual-dispatch refs, require tag/version equality, and use locked desktop dependency resolution. Current desktop arguments contain no `--locked`, unlike the CLI build. (`.github/workflows/release.yml:21`, `.github/workflows/release.yml:83`, `.github/workflows/release.yml:135`)

3. **Finish verification of the actual candidate — M.**

   **F:** The checkout acquired the Wolf/YU-RIS encoding and rejected-provider-usage fixes during this audit. Their ledger records focused verification, while `INT-15` remains `in_progress`. Do not report those two defects as still open, or treat older integration results as proof for the new candidate. (`odd/tasks/cycle-2026-11-01.md:75`, `odd/tasks/cycle-2026-11-01.md:76`, `odd/tasks/cycle-2026-11-01.md:77`)

   **F:** Existing UI acceptance explicitly excludes native Tauri, live providers, real games, runtime, fonts, and overall visual correctness. (`docs/UI-ACCEPTANCE-2026-10-25.md:48`)

   **H — Edit:** A new `docs/RELEASE-ACCEPTANCE-0.3.0.md`; reconcile `odd/tasks/cycle-2026-11-01.md`.

   **H — Acceptance:** Exact-candidate Windows/Ubuntu CI; successful five-platform packaging and four-platform CLI inventory; native install/open/save/reopen smoke checks; older-version updater acceptance. Record untested platforms and game coverage explicitly. The existing release checklist already requires exact-SHA CI and native updater acceptance. (`RELEASE.md:35`, `RELEASE.md:49`, `RELEASE.md:79`)

4. **Replace startup storage panics with actionable errors — M.**

   **F:** Production startup uses `expect` when creating the data directory and opening `project.db`. Desktop calls this before constructing its window; CLI server startup uses the same function. An unwritable data directory or invalid database can terminate startup immediately. This is a conditional source-proven path, not a reproduced first-run failure. (`crates/server/src/lib.rs:372`, `apps/desktop/src-tauri/src/main.rs:21`, `crates/cli/src/commands/server.rs:2`)

   **F:** Desktop socket/runtime initialization also contains startup `expect` paths. (`apps/desktop/src-tauri/src/main.rs:27`, `apps/desktop/src-tauri/src/main.rs:39`)

   **H — Edit:** `crates/server/src/lib.rs`, `apps/desktop/src-tauri/src/main.rs`, `crates/cli/src/commands/server.rs`.

   **H — Acceptance:** Unwritable storage, malformed startup DB, and listener failure produce a visible diagnostic and controlled exit or recovery path; existing files remain untouched.

5. **Complete version and release-note preparation — S.**

   **F:** Current primary manifests consistently say **0.2.0**. This is not an existing version mismatch. However, updater JSON derives its version from the tag, independently of application manifests; tagging `v0.3.0` without bumping them would create inconsistent release metadata. (`Cargo.toml:13`, `apps/desktop/package.json:3`, `apps/desktop/src-tauri/tauri.conf.json:4`, `scripts/generate-updater-manifest.py:91`)

   **F:** `[Unreleased]` is empty. (`CHANGELOG.md:8`)

   **H — Edit:** `Cargo.toml`; the six workspace package versions in `Cargo.lock`; root versions in `apps/desktop/package.json` and `apps/desktop/package-lock.json`; `apps/desktop/src-tauri/tauri.conf.json`; `CHANGELOG.md`. These are the repository’s prescribed version surfaces. (`RELEASE.md:21`)

   **H — Acceptance:** All application versions equal `0.3.0`, tag equals `v0.3.0`, dependency resolutions remain unchanged, and release notes describe the actual release. Replace the workflow’s generic release body with the dated changelog section. (`.github/workflows/release.yml:79`)

6. **Resolve security disposition and dangerous usage guidance — M.**

   **F:** The last comparable dependency report retains **6 vulnerability records / 12 warnings** after withdrawal of a ZIP migration that changed archive interpretation. Application manifests still use ZIP 2. This is recorded audit evidence, not a fresh scan or proof of exploitability. (`docs/B2-REMEDIATION-2026-10-26.md:102`, `crates/core/Cargo.toml:29`, `crates/formats/Cargo.toml:23`)

   **H — Edit:** New `docs/RELEASE-SECURITY-0.3.0.md`; `USAGE.md`.

   **H — Acceptance:** Record each remaining finding’s shipped-platform reachability, mitigation, and disposition. Do not repeat the rejected ZIP upgrade blindly. Correct the guide’s Direct-default, multiple-language, and unconditional cost-limit claims before distribution; details appear below. (`docs/B2-REMEDIATION-2026-10-26.md:100`, `USAGE.md:59`, `USAGE.md:89`, `USAGE.md:107`)

**2. Should-fix soon after**

| Priority | Evidence | H — Files and action | H — Acceptance | Size |
|---|---|---|---|---|
| OS signing | **F:** Apple signing and Windows certificate configuration are null; this is separate from updater signing. (`apps/desktop/src-tauri/tauri.conf.json:40`, `apps/desktop/src-tauri/tauri.conf.json:44`, `RELEASE.md:16`) | `.github/workflows/release.yml`, `tauri.conf.json`, `RELEASE.md`: configure owner-supplied signing/notarization credentials. | Verify native installer signatures and notarization on supported systems. External account waiting is separate. | M |
| Desktop CSP | **F:** Release CSP is disabled with `null`; this alone does not prove an exploitable injection. (`apps/desktop/src-tauri/tauri.conf.json:23`) | `tauri.conf.json`: introduce a restrictive policy compatible with Tauri and the allocated loopback API. | Native navigation, HTTP/WS operations, dialogs, and updater continue working under the policy. | M |
| Compatibility contract | **F:** Patch manifests have schema version 1, while project DB initialization performs migrations and can replace incompatible legacy recording tables. (`crates/core/src/patch/manifest.rs:60`, `crates/core/src/database/schema.rs:14`, `crates/core/src/database/schema.rs:33`) | New `docs/COMPATIBILITY.md`: define supported API, DB upgrade/downgrade, patch-schema, and rollback guarantees. | Publish a concrete compatibility matrix and refusal behavior before promising 1.0 stability. | M |

**3. Version recommendation**

**H — Choose 0.3.0.** The changes add substantial user-facing functionality, making a minor release appropriate, while current evidence does not support a broad 1.0 production-stability promise. The project already declares Semantic Versioning. (`CHANGELOG.md:5`)

- **F:** Twelve formats remain explicitly Experimental; the three Stable plugins inherit the trait’s default stability designation. These labels are not themselves runtime certification. (`docs/ARCHITECTURE.md:50`, `crates/core/src/extraction.rs:102`)
- **F:** Godot 4.4/v3 coverage remains synthetic, with no verified real 4.4/v3 game or engine launch. (`docs/GODOT-DIALOGIC.md:25`)
- **F:** Native UI and comprehensive real-game acceptance remain outside the recorded UI proof. (`docs/UI-ACCEPTANCE-2026-10-25.md:52`)
- **H:** Reserve 1.0 for a deliberately bounded support promise: named stable engines/platforms, native installer/updater proof, representative gameplay checks, and documented API/data compatibility. Experimental plugins need not prevent 1.0 forever, but their boundaries must remain explicit. (`CHANGELOG.md:40`, `RELEASE.md:79`)

**4. Proposed CHANGELOG text**

**F:** Comparing `git log v0.2.0..HEAD --oneline` with the empty section shows that the following user-facing work is missing. The cited source establishes each behavior; this audit does not independently certify its runtime acceptance. (`CHANGELOG.md:8`)

**H — Keep `[Unreleased]`, then add `[0.3.0] - <actual release date>`.** The following is proposed wording; remove audit annotations and citations when applying it.

**Added**

- **F:** Project and QA dashboards, plus a speaker workspace with project-specific character notes. (`apps/desktop/src/App.tsx:42`, `apps/desktop/src/App.tsx:45`, `apps/desktop/src/App.tsx:46`, `crates/server/src/lib.rs:594`)
- **F:** Saved batch queues, translation revision history, and restoration of earlier text. (`apps/desktop/src/lib/queuePersistence.ts:5`, `apps/desktop/src/components/StringHistory.tsx:23`, `odd/tasks/cycle-2026-11-01.md:86`)
- **F:** Fuzzy translation-memory suggestions. (`crates/core/src/database/memory.rs:14`)
- **F:** Configurable model context windows and pricing, with prospective translation estimates. (`crates/core/src/config.rs:16`, `apps/desktop/src/pages/settings/ProvidersSection.tsx:164`, `apps/desktop/src/components/TranslationModal.tsx:201`)
- **F:** TMX catalogs in desktop and HTTP workflows, scoped exports, and import previews with language checks. (`apps/desktop/src/components/ExportModal.tsx:363`, `crates/server/src/lib.rs:637`, `crates/server/src/lib.rs:641`, `docs/TMX-CATALOGS.md:44`)
- **F:** A guided fast workflow, translation of selected files or rows, and RPG Maker text-fit estimates with font refresh. (`apps/desktop/src/lib/fastWorkflow.ts:17`, `odd/tasks/cycle-2026-11-01.md:16`, `docs/IMPROVEMENT-GOAL.md:118`)
- **F:** Warnings for changed visible numbers and inconsistent translations, plus file-scoped review. (`crates/core/src/validation.rs:665`, `crates/core/src/validation.rs:792`, `odd/tasks/cycle-2026-11-01.md:20`)
- **F:** Experimental Godot PCK v3 support and Dialogic character-name translation; additional RPG Maker fields, numbered NScripter scripts, and HTML textarea text. (`crates/formats/src/godot_pck.rs:139`, `crates/formats/src/godot_dialogic.rs:200`, `crates/formats/src/rpgmaker_mv.rs:18`, `crates/formats/src/nscripter.rs:199`, `crates/formats/src/html_game.rs:428`)
- **F:** Backup restoration previews and faster patch compression options. (`crates/server/src/routes/backups.rs:60`, `crates/core/src/patch/pack.rs:20`)

**Changed**

- **F:** Improved translation, backup, glossary, database, and editor performance. (`docs/IMPROVEMENT-GOAL.md:118`, `odd/tasks/cycle-2026-11-01.md:17`, `odd/tasks/cycle-2026-11-01.md:35`)
- **F:** Improved compact layouts, keyboard editing, screen-reader support, and English/Spanish guidance. (`docs/UI-KEYBOARD-EDITING.md:1`, `odd/TASKS.md:9`)

**Fixed**

- **F:** Preserved saved translations during incomplete extraction and changing Unity identifier collisions. (`crates/core/src/project.rs:475`, `crates/core/src/database/strings.rs:1083`)
- **F:** Preserved queue ownership across connection loss and reported actual patch outcomes after cancellation. (`apps/desktop/src/stores/queueStore.ts:143`, `crates/server/src/routes/jobs.rs:58`)
- **F:** Prevented stale approvals, glossary edits, and exports from affecting newer text or another project. (`odd/tasks/cycle-2026-11-01.md:19`, `odd/tasks/cycle-2026-11-01.md:61`, `odd/tasks/cycle-2026-11-01.md:64`)
- **F:** Preserved review drafts, literal replacement text, multiline PO comments, and editor pagination. (`odd/tasks/cycle-2026-11-01.md:84`, `odd/tasks/cycle-2026-11-01.md:85`, `odd/tasks/cycle-2026-11-01.md:60`, `odd/tasks/cycle-2026-11-01.md:73`)
- **F:** Improved provider retry classification, per-row Ollama context, DeepL response validation, cost preflight, and CLI cancellation. (`odd/tasks/cycle-2026-11-01.md:79`, `crates/providers/src/ollama.rs:96`, `odd/tasks/cycle-2026-11-01.md:59`, `odd/tasks/cycle-2026-11-01.md:66`, `odd/tasks/cycle-2026-11-01.md:63`)
- **F:** Retained reported tokens and cost when paid-provider answers are rejected. (`crates/providers/src/openai.rs:287`, `crates/providers/src/openai.rs:308`, `odd/tasks/cycle-2026-11-01.md:76`)
- **F:** Refused Wolf RPG and YU-RIS translations that change characters during Shift-JIS encoding. (`crates/formats/src/wolf_rpg.rs:475`, `crates/formats/src/yuris.rs:406`)

**Security**

- **F:** Hardened archive paths, bounded malformed archive reads, validated patch-download redirects, and updated several vulnerable dependencies. Remaining dependency findings are not all resolved. (`odd/tasks/cycle-2026-11-01.md:28`, `odd/tasks/cycle-2026-11-01.md:36`, `crates/cli/src/commands/patch.rs:284`, `docs/B2-REMEDIATION-2026-10-26.md:102`)

**H:** Retain a short Known Issues note covering Experimental formats, unverified native/gameplay coverage, remaining dependency findings, and unsigned OS installers. Do not announce signing or updater success until acceptance is recorded. (`CHANGELOG.md:40`, `RELEASE.md:16`, `RELEASE.md:57`)

**5. Docs corrections**

Each replacement below is **H**, based on the **F** implementation evidence cited.

| Current location | Corrected text |
|---|---|
| `README.md:66` | “Selected standalone Godot 4.0–4.4 PCK v2 and Godot 4.5+ PCK v3; Experimental. Newer profiles have synthetic-fixture coverage only.” (`crates/formats/src/godot_pck.rs:139`, `docs/GODOT-DIALOGIC.md:25`) |
| `README.md:148` | Remove “no Settings editor or estimate UI is added here.” Both interfaces now exist. (`apps/desktop/src/pages/settings/ProvidersSection.tsx:164`, `apps/desktop/src/components/TranslationModal.tsx:632`) |
| `README.md:236`, `README.md:248` | “TMX project catalogs are available through CLI, desktop, and HTTP. The restricted plaintext profile still applies.” (`apps/desktop/src/components/ExportModal.tsx:363`, `crates/server/src/lib.rs:637`) |
| `README.md:343` | Replace the old Ollama prompt description with “Ollama sends context and glossary with each input record.” Remove “speaker workspace UI is a separate follow-up.” (`crates/providers/src/ollama.rs:96`, `crates/providers/src/openai.rs:154`, `apps/desktop/src/App.tsx:46`) |
| `USAGE.md:31`, `USAGE.md:33` | Include VX in the Stable RPG Maker family and Godot/Dialogic in Experimental. (`README.md:39`, `crates/formats/src/lib.rs:43`, `docs/GODOT-DIALOGIC.md:3`) |
| `USAGE.md:49` | “Defaults use detected project text, saved choices, and settings. Confirm both languages before starting.” (`apps/desktop/src/lib/translationDefaults.ts:134`) |
| `USAGE.md:59` | “Cost limits use estimates and observed usage; unknown cost stops further limited calls. They cannot guarantee the provider’s final bill.” (`crates/core/src/translation/manager.rs:527`, `README.md:114`) |
| `USAGE.md:73` | “Disappeared rows are removed after complete extraction; incomplete extraction preserves absent saved rows.” (`crates/core/src/project.rs:475`) |
| `USAGE.md:89` | “Select the single target language represented by this project database.” (`crates/core/src/extraction.rs:947`) |
| `USAGE.md:107` | “Replace-only engines default to a translated copy. Direct writes in place and must be chosen explicitly.” (`apps/desktop/src/lib/injectModes.ts:28`) |
| `USAGE.md:108` | “Pack uses recorded injection output; Direct is not required.” (`README.md:266`, `docs/IMPROVEMENT-GOAL.md:52`) |
| `docs/GODOT-DIALOGIC.md:15` | “Literal character display names can be translated; character identities remain separate.” (`crates/formats/src/godot_dialogic.rs:200`, `crates/formats/src/godot_dialogic.rs:335`) |
| `CLAUDE.md:131` | Include `tmx` in export/import examples, with explicit TMX source language. (`README.md:241`, `docs/TMX-CATALOGS.md:89`) |
| `docs/ARCHITECTURE.md:44` | Point to `database/schema.rs` and describe transactional migrations plus history/content-revision tables, replacing obsolete source anchors. (`crates/core/src/database/schema.rs:14`, `crates/core/src/database/schema.rs:151`, `crates/core/src/database/schema.rs:192`) |
| `RELEASE.md:47` | Replace hard-coded `v0.2.0` with the intended release version/tag. (`RELEASE.md:21`) |

**F — Ports and provider registration largely match current README guidance:** CLI server defaults to 7842, Vite uses 1420, desktop allocates a loopback port, and local providers are registered even when their separate services are unavailable. (`crates/cli/src/main.rs:648`, `apps/desktop/vite.config.ts:8`, `apps/desktop/src-tauri/src/main.rs:28`, `crates/providers/src/lib.rs:124`, `crates/providers/src/lib.rs:237`)

**H — Installer filenames require uploaded-asset confirmation.** The CLI names are explicitly wired, while desktop updater discovery matches filename suffixes. This audit found no proved desktop filename mismatch; retain the asset-inventory acceptance gate. (`.github/workflows/release.yml:96`, `scripts/generate-updater-manifest.py:16`, `README.md:158`)

**6. VPS/domain/Cloudflare deployment notes**

| Option | F — Repository basis | H — Prerequisites and implementation |
|---|---|---|
| Static download/docs site | The project records no Locust web deployment; Axum is an API, and the desktop frontend targets a local backend. (`docs/IMPROVEMENT-GOAL.md:25`, `apps/desktop/src/lib/api.ts:46`) | Owner supplies domain/zone control, chosen host, public VPS IP if applicable, A/AAAA or host CNAME records, TLS, and scoped deployment credentials. Build a separate public site linking verified release assets and docs. Do not present the desktop frontend as a hosted translation service. |
| Updater endpoint mirror | Endpoint is configurable; generator currently hard-codes GitHub artifact URLs. (`apps/desktop/src-tauri/tauri.conf.json:63`, `scripts/generate-updater-manifest.py:84`) | Owner supplies an update hostname, DNS, TLS, static/object storage, and scoped CI upload credentials. Change `plugins.updater.endpoints` to an HTTPS JSON URL. Add workflow upload/promotion after release verification. A manifest-only mirror may retain GitHub artifact URLs; a full mirror also needs generator URL configuration and exact signed artifact copies. |
| Patch hosting | `apply --url` downloads HTTP(S), checks every redirect’s addresses, refuses private destinations and HTTPS downgrades, and enforces download limits. (`crates/cli/src/commands/patch.rs:267`, `crates/cli/src/commands/patch.rs:284`, `crates/core/src/patch/net_policy.rs:14`, `crates/core/src/patch/zipsec.rs:235`) | Owner supplies a patch hostname, public DNS, TLS, storage capacity/bandwidth, scoped upload credentials, and permission to distribute each patch. Serve immutable versioned ZIP URLs without interactive login/challenge pages. Configure Cloudflare to deliver the actual ZIP/JSON bytes. |

**H — Updater migration:** Ship an overlap period with both endpoints and keep the existing GitHub endpoint working for previously installed applications. Retain the public signing key; changing hostname does not require changing signing identity. Publish versioned assets first, then atomically promote `latest.json`, with short cache lifetime or explicit purge. (`apps/desktop/src-tauri/tauri.conf.json:62`, `apps/desktop/src-tauri/tauri.conf.json:64`, `scripts/generate-updater-manifest.py:89`)

**F — What stays local:** Desktop binds `127.0.0.1:0`; normal server startup binds `127.0.0.1:<port>`. The outer guard rejects foreign Host and Origin headers before handlers run. (`apps/desktop/src-tauri/src/main.rs:28`, `crates/server/src/lib.rs:662`, `crates/server/src/lib.rs:566`, `crates/server/src/lib.rs:654`)

**F — Important boundary:** A library function can explicitly bind another address, and missing Host/Origin headers are not authentication. The guard is local-client protection, not a public-service authorization system. (`crates/server/src/lib.rs:671`, `crates/server/src/lib.rs:567`)

**H — Do not expose Axum through a public listener, reverse proxy, or Cloudflare Tunnel.** Its operations accept local filesystem paths and perform patch writes; a proxy rewriting Host to localhost would undermine the intended boundary. Keep project databases, credentials, backups, and translation operations on the user’s machine. (`crates/server/src/lib.rs:663`, `README.md:349`, `README.md:359`)

**7. Bounded writer units — two lanes**

**H — Proposed ownership:** These units have disjoint named files. Durations are hands-on bounds; owner credential, account, or hardware availability is external. Do not reopen the two fixes already recorded as complete. (`odd/tasks/cycle-2026-11-01.md:75`)

| Lane | Unit | Exclusive files | Bound and completion |
|---|---|---|---|
| Release/docs | **A1 — Publication gate** | `.github/workflows/release.yml`; `scripts/generate-updater-manifest.py`; `tools/test_updater_manifest.py`; `RELEASE.md`; `docs/update-server.md` | ≤3 days. Draft/finalizer flow, version/ref checks, locked desktop builds, inventory validation, changelog-derived body. |
| Release/docs | **A2 — Release metadata and desktop policy** | `Cargo.toml`; `Cargo.lock`; `apps/desktop/package.json`; `apps/desktop/package-lock.json`; `apps/desktop/src-tauri/tauri.conf.json`; `CHANGELOG.md` | ≤2 days. Synchronize 0.3.0 and notes without dependency churn; assess CSP change under native acceptance. |
| Release/docs | **A3 — Correct user guidance** | `README.md`; `USAGE.md`; `CLAUDE.md`; `docs/ARCHITECTURE.md`; `docs/GODOT-DIALOGIC.md` | ≤2 days. Apply the corrections above and verify each command/claim against source. |
| Release/docs | **A4 — Deployment specification** | New `docs/DEPLOYMENT.md` | ≤1 day. Record chosen domains, DNS/TLS/storage, scoped credentials, cache/promotion policy, and local-only boundary. |
| Runtime/assurance | **B1 — Startup errors** | `crates/server/src/lib.rs`; `apps/desktop/src-tauri/src/main.rs`; `crates/cli/src/commands/server.rs` | ≤2 days. Controlled storage/listener failures with actionable diagnostics and preservation checks. |
| Runtime/assurance | **B2 — Security disposition** | New `docs/RELEASE-SECURITY-0.3.0.md` | ≤2 days. Per-finding shipped reachability and mitigation; explicitly retain unresolved ZIP constraints. |
| Runtime/assurance | **B3 — Candidate acceptance** | New `docs/RELEASE-ACCEPTANCE-0.3.0.md`; `odd/tasks/cycle-2026-11-01.md`; `odd/tasks/release-v0.2.0.md` | ≤3 days. Finish integration evidence, reconcile signing history, record platform inventories and native updater outcomes. |
| Runtime/assurance | **B4 — Compatibility promise** | New `docs/COMPATIBILITY.md` | ≤1 day. Define API, project DB, patch-schema, upgrade/downgrade, and rollback guarantees. |