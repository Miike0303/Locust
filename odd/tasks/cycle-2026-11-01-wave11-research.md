# Cycle 2026-11-01 — wave 11 research (release-acceptance verification gaps)

Two independent Codex gpt-6.1-sol high researchers, read-only. Both propose: gate the existing production-browser capture in CI and extend it to a full user-driven lifecycle (`tools/capture_ui.mjs`, parent confirmed it exists), a desktop executable startup self-test, a complete Stable-format CLI lifecycle script, throwaway-key signer/manifest verification, a secretless bundle dry run with static inventory, and a real updater loopback client. `.github/workflows/ci.yml` is a shared file: the parent wires CI jobs after each writer.

## Researcher A

**F — The gaps are narrower than the release audit originally described.** Publication gating, version synchronization, startup-error handling, and release documentation are recorded as complete; they should not become new implementation tasks. (`odd/tasks/cycle-2026-11-01.md:79`, `odd/tasks/cycle-2026-11-01.md:80`, `odd/tasks/cycle-2026-11-01.md:81`, `odd/tasks/cycle-2026-11-01.md:82`)

**F — CI already exercises substantial synthetic acceptance:** frontend builds, workspace tests, Python pipelines, RGSS oracles, and offline updater-manifest tests on Ubuntu and Windows. It does **not explicitly bundle the desktop or run its executable**. (`.github/workflows/ci.yml:56`, `.github/workflows/ci.yml:61`, `.github/workflows/ci.yml:76`, `.github/workflows/ci.yml:84`, `.github/workflows/ci.yml:98`, `.github/workflows/ci.yml:136`, `.github/workflows/ci.yml:176`)

**H — Recommended eight units follow.** All sizes, durations, risks, acceptance checks, and CI costs are estimates, not measured results. S means approximately one writer-day; M means at most two. Costs are additional elapsed minutes per platform, assuming existing dependency/build caches. Test traffic should remain on loopback; missing prerequisites should fail explicitly.

For compact file lists, **D** means `apps/desktop/src-tauri`, and **W** means `.github/workflows`.

1. **W11-01 — Promote the existing production-browser capture to CI**

   **H — S; low risk. Value:** Turn the existing reproducible browser proof into a required, candidate-specific Windows/Ubuntu gate.

   **F — Exists today:** `capture_ui.mjs` is a dependency-free Node/CDP runner, serves unmodified `dist` assets, starts a real CLI server, generates synthetic MV fixtures, records 44 captures, and checks browser errors and cleanup. Chrome defaults to a Windows installation path; `CHROME_PATH` is already configurable. (`tools/capture_ui.mjs:3`, `tools/capture_ui.mjs:24`, `tools/capture_ui.mjs:149`, `tools/capture_ui.mjs:179`, `tools/capture_ui.mjs:470`, `tools/capture_ui.mjs:476`, `tools/capture_ui.mjs:506`, `tools/capture_ui.mjs:440`)

   **H — Files:** Edit `tools/capture_ui.mjs` and `W/ci.yml`.

   **H — Acceptance:** Run after existing CLI/frontend builds on both runners; resolve an installed Chrome executable without downloading a browser during the test. Require all 44 captures, zero unexpected browser/API errors, immutable frontend fingerprints, stopped owned processes, and successful scratch cleanup. Upload bounded logs and the manifest on failure.

   **H — CI cost:** Ubuntu 1–3 minutes; Windows 2–4. Shares both files with W11-02 and the workflow with W11-03/04.

2. **W11-02 — Add one complete user-driven production-browser lifecycle**

   **H — M; medium risk. Value:** Prove that the shipped frontend can complete the actual translation-to-patch workflow against production backend handlers.

   **F — Exists today:** The capture runner opens/seeds projects through HTTP, writes translations directly through HTTP, previews export controls, and invokes validation directly; its fast-workflow scenario opens and closes the dialog. Those scenarios do not execute the requested complete workflow. (`tools/capture_ui.mjs:193`, `tools/capture_ui.mjs:198`, `tools/capture_ui.mjs:262`, `tools/capture_ui.mjs:281`, `tools/capture_ui.mjs:329`)

   **F — Important missing transport seam:** Production browser HTTP uses port 7842, and job WebSockets independently use that port. The runner currently redirects HTTP requests to its allocated backend. (`apps/desktop/src/lib/api.ts:46`, `apps/desktop/src/lib/api.ts:1291`, `apps/desktop/src/lib/api.ts:1296`, `tools/capture_ui.mjs:493`)

   **F — No provider account is necessary:** The registered mock provider requires no API key. (`crates/providers/src/lib.rs:122`, `crates/providers/src/mock.rs:233`)

   **H — Files:** Edit `tools/capture_ui.mjs`, `W/ci.yml`; create `tools/ui_release_lifecycle.mjs`.

   **H — Acceptance:** Using one synthetic MV project, drive visible controls through open → mock translate → save an edit → validate → export → inject into an owned copy → package → apply to a second pristine copy → rollback. Observe real job completion over WebSockets. Verify exported text, changed injected payload, and exact restoration of original game-file paths and hashes. Backend requests may inspect outcomes, but must not perform the workflow stages being tested. Provide loopback routing for both HTTP and WebSocket traffic without rewriting API responses.

   **H — CI cost:** Ubuntu 1–3 minutes; Windows 2–4, reusing W11-01’s processes. Shares files with W11-01.

3. **W11-03 — Complete the Stable dialect matrix behind one CLI command**

   **H — M; medium risk. Value:** Close missing dialect/provider-path coverage without rebuilding acceptance already present.

   **F — Exists today:** Stable coverage comprises MV/MZ, XP/VX/VX Ace, and Ren’Py. Existing Rust lifecycle tests already translate, inject, package, apply, and verify byte-identical rollback for Ren’Py and MV. (`docs/COMPATIBILITY.md:22`, `crates/cli/tests/ja_en_es_pipeline.rs:25`, `crates/cli/tests/ja_en_es_pipeline.rs:724`, `crates/cli/tests/ja_en_es_pipeline.rs:744`, `crates/cli/tests/ja_en_es_pipeline.rs:791`)

   **F — VX Ace also already has a public CLI lifecycle**, including apply/rollback and an independent wire oracle, but translations are seeded by export/import. Its fixture profiles are Ace-only. (`tools/test_qa_rgss_oracle.py:333`, `tools/test_qa_rgss_oracle.py:395`, `tools/qa_rgss_oracle.py:162`, `tools/qa_rgss_oracle.py:166`)

   **H — Files:** Create `tools/stable_cli_acceptance.py`; edit `tools/qa_rgss_oracle.py` and `W/ci.yml`.

   **H — Acceptance:** A single command accepts an existing CLI path and reports six explicit cases: MV, MZ, XP, VX, VX Ace, Ren’Py. Reuse fixture shapes and Marshal wire helpers; add genuine XP/VX command layouts rather than merely renaming Ace extensions. Each case executes extract → `translate --provider mock` → validate → inject → patch → apply → rollback. Require nonzero translations/writes, unchanged nontext sentinels, expected translated slots, and exact pristine-file restoration. Do not rerun the entire existing pipeline suite inside this command.

   **H — CI cost:** Ubuntu 1–3 minutes; Windows 2–5. Shares `W/ci.yml` with W11-01/02/04.

4. **W11-04 — Execute the built desktop through a startup self-test**

   **H — M; medium risk. Value:** Catch desktop executable, embedded-state, and loopback-startup failures that compilation alone misses.

   **F — Exists today:** Desktop startup already constructs application state, retains an allocated loopback listener, and starts the server thread before creating Tauri. `main` then proceeds directly into the application builder. (`D/src/main.rs:54`, `D/src/main.rs:60`, `D/src/main.rs:80`, `D/src/main.rs:108`, `D/src/main.rs:110`)

   **H — Files:** Edit `D/src/main.rs` and `W/ci.yml`; create `tools/desktop_startup_smoke.py`.

   **H — Acceptance:** After the existing frontend build, explicitly run `cargo build --locked -p locust-desktop`, then execute the resulting binary with `--self-test`. The flag must reuse `start_backend`, check `/health` and initialized state, emit a small machine-readable result, and exit within a deadline before creating a window. Include an isolated invalid-data-directory case that exits nonzero without awaiting a native dialog. Run on Windows and Ubuntu.

   **H — CI cost:** Ubuntu 1–4 minutes; Windows 2–6 after existing workspace compilation. Shares only the workflow with W11-01/02/03.

5. **W11-05 — Secretless Tauri package-build dry run**

   **H — M; medium risk. Value:** Exercise bundling before owner credentials or a production tag are available.

   **F — Exists today:** Real desktop bundling lives in the release workflow, after draft creation, and receives owner updater-signing secrets. The configured bundle targets are `all`, with updater artifacts enabled. (`W/release.yml:42`, `W/release.yml:118`, `W/release.yml:122`, `D/tauri.conf.json:27`)

   **H — Files:** Create `W/release-smoke.yml` and `tools/tauri_bundle_smoke.py`.

   **H — Acceptance:** On Windows x64 and Ubuntu x64, run a local-only `tauri build --debug … -- --locked`, initially bounded to MSI and Debian packages. Generate an ephemeral signing identity and apply overrides through an owned temporary config; never edit the checked-in public key. Do not invoke release creation/upload/publication. Record effective configuration and exact bundle paths. Preflight cached bundler prerequisites and fail clearly if unavailable; do not silently fetch tools during an offline local run.

   **H — CI cost:** Ubuntu 5–12 minutes; Windows 6–15 with warm caches. Cold builds may take substantially longer. Shares its workflow with W11-06/07/08; may consume W11-07’s key-generation helper without editing it.

6. **W11-06 — Inspect produced package metadata and payload inventories**

   **H — M; medium risk. Value:** Detect wrong package versions, architectures, executable placement, and accidental payload inclusion before publication.

   **F — Exists today:** The final release gate checks uploaded asset names, nonempty sizes, manifest platform coverage, URLs, and signature shape/equality. It does not inspect package contents. (`scripts/release-gate.py:73`, `scripts/release-gate.py:80`, `scripts/release-gate.py:86`, `scripts/release-gate.py:95`)

   **F — Artifact naming contracts already distinguish Windows MSI, macOS application archives, and Linux AppImage.** (`scripts/generate-updater-manifest.py:15`)

   **H — Files:** Create `tools/package_inventory.py`, `tools/test_package_inventory.py`; edit `W/release-smoke.yml` and `W/release.yml`.

   **H — Acceptance:** Inspect MSI metadata/tables and Debian control/archive contents without installing them. Require source-version/product/architecture agreement, expected executable placement, nonempty payloads, and absence of fixture DBs, test keys, scratch paths, and development dependencies. Add bounded AppImage/application-archive inventory adapters for their release jobs. Exercise refusal with wrong-version, missing-executable, and unexpected-file fixtures. Invoke inspection before real bundle upload as well as during dry runs; retain the existing nine-build publication gate.

   **H — CI cost:** 0.5–2 minutes per platform, excluding package creation. Shares workflows with W11-05/07/08; no source overlap with W11-01–04.

7. **W11-07 — Cryptographically verify signer → manifest → payload**

   **H — M; medium risk. Value:** Replace format-only assurance with actual key/signature correspondence using disposable keys.

   **F — Exists today:** Manifest validation explicitly checks signature shape rather than key correspondence. Tests manufacture packet-shaped byte sequences and mock `gh`/`curl`; the release guide explicitly says these checks do not prove cryptographic validity. (`scripts/generate-updater-manifest.py:37`, `tools/test_updater_manifest.py:29`, `tools/test_updater_manifest.py:78`, `RELEASE.md:88`)

   **H — Files:** Edit `scripts/generate-updater-manifest.py`, `tools/test_updater_manifest.py`, `D/Cargo.toml`, `Cargo.lock`, `W/release-smoke.yml`; create `tools/updater_crypto.py` and `D/tests/updater_crypto.rs`.

   **H — Acceptance:** Generate a throwaway key with the installed Tauri signer, sign small opaque payloads representing all five platform names, and generate strict `latest.json` through an offline asset-directory input. Verify every manifest signature against its actual payload and test public key. Reject altered payloads, wrong keys, crossed signatures, and malformed base64. Preserve production URL/pairing behavior and existing atomic-publication refusal tests. Delete test private keys; never upload them.

   **H — CI cost:** 0.5–2 minutes per platform after compilation. Shares Cargo files and workflow with W11-08; shares workflow with W11-05/06.

8. **W11-08 — Exercise the actual updater client against loopback**

   **H — M; medium risk. Value:** Prove that Tauri’s updater consumes the manifest and rejects invalid downloads before any installation.

   **F — Exists today:** The desktop registers `tauri-plugin-updater`, and its configured endpoint is GitHub. Native older-install acceptance remains a separate release requirement. (`D/src/main.rs:113`, `D/tauri.conf.json:60`, `RELEASE.md:115`)

   **H — Files:** Edit `D/Cargo.toml`, `Cargo.lock`, `W/release-smoke.yml`; create `D/tests/updater_acceptance.rs` and `tools/updater_loopback.py`.

   **H — Acceptance:** Construct a test Tauri context with a throwaway public key and test-only loopback endpoint. Feed W11-07’s manifest schema/signatures through the real plugin’s check/download path. Require a newer version to download and verify; current/older versions to produce no update; tampered bytes and wrong-key signatures to fail. Disable proxies, bound requests/timeouts, and reject non-loopback destinations. Stop before installation/relaunch. Keep endpoint/key overrides entirely in the test context.

   **H — CI cost:** 1–3 minutes per platform after compilation. Shares Cargo files and workflow with W11-07.

**H — Common implementation constraints:** Reuse existing build targets, run only one Cargo writer/build at a time locally, disable incremental/debug information for disposable local builds, and keep generated fixtures/reports bounded under owned scratch directories. These choices follow the repository’s shared-target verification practice and guarded cleanup design. (`tools/verify-worktree.sh:9`, `tools/verify-worktree.sh:13`, `tools/cleanup-scratch.ps1:41`)

**F — Acceptance limits remain explicit:** Synthetic scripts do not establish gameplay, native dialog rendering, older-install updater installation, OS signing, or macOS/ARM execution. Those remain outside these proposed checks and are still required or qualified by the release documentation. (`CHANGELOG.md:52`, `CHANGELOG.md:54`, `RELEASE.md:75`, `RELEASE.md:117`, `docs/COMPATIBILITY.md:27`)

**H — Pairing into two disjoint-file lanes:** Lane A owns W11-01 → W11-02, then W11-03 and W11-04, including all `ci.yml` changes. Lane B owns W11-07 → W11-08 and W11-05 → W11-06, including Cargo dependency files and both release workflows. Serialize units sharing files within each lane. The lanes share no edited files.

**H — Ranked table; file sets are exact as listed above. CI minutes are Ubuntu / Windows, incremental and cache-warm.**

| Rank | ID | Title | Size | Files | CI minutes |
|---:|---|---|:---:|---|---:|
| 1 | W11-02 | Complete production-browser lifecycle | M | `capture_ui.mjs`, new `ui_release_lifecycle.mjs`, `W/ci.yml` | 1–3 / 2–4 |
| 2 | W11-07 | Cryptographic signer/manifest proof | M | Generator, manifest tests, new crypto runner/test, `D/Cargo.toml`, `Cargo.lock`, release-smoke workflow | 0.5–2 / 0.5–2 |
| 3 | W11-04 | Desktop executable startup self-test | M | `D/src/main.rs`, new startup runner, `W/ci.yml` | 1–4 / 2–6 |
| 4 | W11-05 | Secretless package-build dry run | M | New bundle runner and `W/release-smoke.yml` | 5–12 / 6–15 |
| 5 | W11-03 | Complete Stable CLI dialect matrix | M | New Stable runner, RGSS oracle, `W/ci.yml` | 1–3 / 2–5 |
| 6 | W11-08 | Real updater loopback client | M | New updater test/runner, Cargo files, release-smoke workflow | 1–3 / 1–3 |
| 7 | W11-06 | Produced-package inventory | M | New inventory runner/tests, both release workflows | 0.5–2 / 0.5–2 |
| 8 | W11-01 | Existing browser capture in CI | S | `tools/capture_ui.mjs`, `W/ci.yml` | 1–3 / 2–4 |

## Researcher B

**H — Recommend these eight units.** Each is bounded to one writer for at most two days. Together they automate production-browser flows, native startup, cryptographic updater checks, package inspection and Stable-format CLI replay.

**F — Do not reopen completed work:** strict publication gating, version preparation, compatibility/security documentation, startup-error handling and UpdateChecker state handling are already recorded as done. (`odd/tasks/cycle-2026-11-01.md:79–82`, `odd/tasks/cycle-2026-11-01.md:97`)

**F — Existing assurance is substantial but bounded:** CI builds the frontend and runs workspace lint/tests on Ubuntu and Windows; desktop packaging occurs in the separate release workflow. The release documentation explicitly distinguishes generated manifests from cryptographic verification and installed-app updater acceptance. (`.github/workflows/ci.yml:81–99`, `.github/workflows/ci.yml:161–177`, `.github/workflows/release.yml:118–134`, `RELEASE.md:88–90`, `RELEASE.md:115–123`)

**H — Estimation convention:** S = roughly one day; M = up to two days. CI estimates below are incremental minutes **per runner**, assuming dependencies and build caches are available. They are estimates, not measured results.

**W11-01 — Promote the existing production-browser capture runner into CI**

**H — S; low risk. Value:** turn existing reproducible browser evidence into an automatic candidate gate.

**F — Today:** `capture_ui.mjs` uses Node 24 without additional packages, serves unmodified production assets, generates synthetic MV fixtures, and launches an isolated real CLI server and Chrome profile. It already fingerprints inputs and cleans up owned processes and temporary files. (`tools/capture_ui.mjs:3–9`, `tools/capture_ui.mjs:149–166`, `tools/capture_ui.mjs:179–216`, `tools/capture_ui.mjs:387–443`, `tools/capture_ui.mjs:458–476`)

**F — Gap:** CI never invokes this runner. Its geometry/focus booleans are recorded, while the final assertions check capture count, browser errors and HTTP statuses rather than those booleans. (`.github/workflows/ci.yml:50–99`, `.github/workflows/ci.yml:131–177`, `tools/capture_ui.mjs:245–249`, `tools/capture_ui.mjs:506–509`)

**H — Files:** edit `tools/capture_ui.mjs` and `.github/workflows/ci.yml`.

**H — Acceptance:**

- Run after the existing frontend/CLI builds on both platforms, setting `CARGO_TARGET_DIR=target` explicitly.
- Resolve an existing Chrome executable through `CHROME_PATH`; missing prerequisites fail.
- Require all current 44 captures, every geometry/focus check, expected HTTP outcomes and successful cleanup.
- Upload screenshots, logs and the SHA-bound manifest on failure.
- Reject external requests during scenarios; retain genuine backend responses.

**H — CI:** Ubuntu 1–3 minutes; Windows 1–3. **Shares files:** W11-02/03 share the runner; W11-08 shares CI.

**W11-02 — Drive the browser authoring workflow through completion**

**H — M; medium risk, principally WebSocket routing and download synchronization. Value:** prove that production UI actions produce persisted translations and an actual exported catalog.

**F — Today:** the capture fixture opens projects and seeds translations through API PATCH requests. Its scenarios inspect export controls and trigger validation through a direct API call; they do not perform the requested mock-translation authoring sequence. Browser export creates a Blob download. (`tools/capture_ui.mjs:193–206`, `tools/capture_ui.mjs:281–297`, `tools/capture_ui.mjs:329–338`, `apps/desktop/src/lib/api.ts:930–942`)

**F — Important seam:** production browser HTTP and WebSocket URLs target port 7842, but the current runner redirects only HTTP requests to its random backend port. Mock translation requires no API key and produces results locally. (`apps/desktop/src/lib/api.ts:35–46`, `apps/desktop/src/lib/api.ts:1285–1296`, `tools/capture_ui.mjs:493–500`, `crates/providers/src/mock.rs:233–249`)

**H — Files:** edit `tools/capture_ui.mjs` only.

**H — Acceptance:**

- Add a selectable workflow scenario using fresh, initially untranslated synthetic MV data.
- Through UI controls: open project → select Mock → translate → await genuine WebSocket completion → edit and save one translation → reload → validate → download PO.
- Read the saved catalog from disk and verify source identities and the edited translation.
- Record WebSocket connection/completion evidence.
- Use an explicitly owned backend on 7842 for this scenario; refuse an occupied port before contacting it. Preserve the existing random-port capture mode.

**H — CI:** Ubuntu 1–2 minutes; Windows 1–3. **Shares files:** W11-01/03. Depends on W11-01’s CI integration.

**W11-03 — Drive browser injection, packaging, apply and rollback**

**H — M; medium risk from asynchronous jobs and filesystem assertions. Value:** verify the complete distribution workflow across UI, HTTP, WebSocket and filesystem boundaries.

**F — Today:** the UI already implements patch packaging, asynchronous apply with job subscription, rollback preview and rollback. It exposes local ZIP paths, so this smoke need not exercise a remote patch URL. (`apps/desktop/src/components/PatchModal.tsx:474–528`, `apps/desktop/src/components/PatchModal.tsx:585–620`, `apps/desktop/src/components/PatchModal.tsx:654–690`, `apps/desktop/src/components/patch/PatchApplyTab.tsx:78–96`)

**F — Existing lower-level proof:** CLI lifecycle tests already compare installed artifacts and verify exact rollback bytes; the browser capture scenarios currently stop without exercising injection or patch distribution. (`crates/cli/tests/ja_en_es_pipeline.rs:743–800`, `tools/capture_ui.mjs:253–369`)

**H — Files:** edit `tools/capture_ui.mjs` only.

**H — Acceptance:**

- Continue W11-02’s synthetic project through UI-selected Replace injection.
- Package a strict patch using recorded output and the pristine fixture.
- Apply the local ZIP to a separate pristine consumer directory through the UI; await genuine patch-job completion.
- Compare consumer payload paths and hashes against injected output.
- Preview rollback and prove no payload changes; execute rollback and require the original payload inventory and hashes, with no extra translated files.
- Assert the pristine source remains unchanged throughout.

**H — CI:** Ubuntu 1–3 minutes; Windows 2–4. **Shares files:** W11-01/02; depends on W11-02.

**W11-04 — Build and execute the native desktop binary’s startup self-test**

**H — M; medium risk around startup readiness and process lifetime. Value:** prove that a linked desktop executable can initialize its real state and embedded server on both CI platforms.

**F — Today:** CI already compiles/checks desktop workspace targets, so this is an executable smoke gap rather than an absence of Rust coverage. Desktop startup constructs real application state, reserves loopback port zero and starts the server before constructing Tauri. `main` then enters the normal application path without a self-test branch. (`.github/workflows/ci.yml:28–33`, `.github/workflows/ci.yml:95–99`, `apps/desktop/src-tauri/src/main.rs:54–108`, `apps/desktop/src-tauri/src/main.rs:110–154`)

**H — Files:**

- Edit `apps/desktop/src-tauri/src/main.rs`.
- Create `apps/desktop/src-tauri/src/self_test.rs`.
- Create `tools/qa_desktop_startup.py`.
- Create `.github/workflows/desktop-smoke.yml`.

**H — Acceptance:**

- Build the frontend, then `cargo build --locked -p locust-desktop`.
- Execute `locust-desktop --self-test` under an isolated data/config/backup profile.
- Reuse production backend initialization; require successful `/health`, format and provider queries before emitting a machine-readable receipt and exiting zero.
- Enforce a short deadline and verify the process/socket exits.
- Include a malformed startup DB case: controlled nonzero exit, preserved input bytes and no dialog wait in self-test mode.

**H — CI:** Ubuntu 3–8 minutes; Windows 4–10, including incremental compilation. **Shares files:** W11-05/06/07 share the new workflow. This does not repeat the completed startup-error fix.

**W11-05 — Generate real throwaway signatures and verify manifest correspondence**

**H — M; low-to-medium risk from signature encoding and test-key handling. Value:** prove signer → `.sig` → production manifest generator → cryptographic verification without owner credentials.

**F — Today:** the generator explicitly validates signature **shape**, and the release gate explicitly excludes cryptographic key correspondence. Existing tests manufacture correctly shaped packets from repeated bytes. The desktop package includes Tauri CLI, and `minisign-verify` is already locked. (`scripts/generate-updater-manifest.py:37–50`, `scripts/release-gate.py:73–78`, `tools/test_updater_manifest.py:29–33`, `apps/desktop/package.json:38`, `Cargo.lock:3488–3491`)

**H — Files:**

- Create `scripts/qa-updater-signatures.py`.
- Create `apps/desktop/src-tauri/tests/updater_signature_contract.rs`.
- Edit `apps/desktop/src-tauri/Cargo.toml`, `Cargo.lock` and `.github/workflows/desktop-smoke.yml`.

**H — Acceptance:**

- Generate a fresh disposable keypair using Tauri signer and sign five small, distinct synthetic artifact payloads.
- Run the existing production generator with only its GitHub/download transport replaced by local-file responses—the existing tests demonstrate that transport seam. (`tools/test_updater_manifest.py:50–81`)
- Require strict five-platform output and exact `.sig` correspondence.
- Cryptographically verify every manifest signature against its payload and disposable public key.
- Require rejection for a changed payload, wrong key and swapped platform signatures.
- Keep private keys inside owned scratch and out of uploaded evidence.

**H — CI:** Ubuntu 1–3 minutes; Windows 1–3, including incremental verifier compilation. **Shares files:** W11-06 shares Cargo metadata; W11-04/06/07 share the workflow.

**W11-06 — Exercise the actual Rust updater’s check/download path**

**H — M; medium risk from plugin configuration and version selection. Value:** test the native updater parser and verified download path beyond JavaScript mocks.

**F — Today:** UpdateChecker’s unit test replaces the updater module with a stub. The production desktop registers the native updater plugin. (`apps/desktop/src/components/UpdateChecker.test.tsx:28–37`, `apps/desktop/src-tauri/src/main.rs:110–114`)

**F — Feasibility:** the locally cached updater implementation exposes `download()` separately from installation and verifies the signature before returning bytes; cached Tauri provides a `MockRuntime` builder. These are dependency-source observations, not executed acceptance results. (`C:/Users/Mike/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-plugin-updater-2.10.1/src/updater.rs:648–729`, `C:/Users/Mike/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-2.10.3/src/test/mod.rs:150–166`)

**H — Files:** create `apps/desktop/src-tauri/tests/updater_download_contract.rs`; edit `apps/desktop/src-tauri/Cargo.toml`, `Cargo.lock` and `.github/workflows/desktop-smoke.yml`.

**H — Acceptance:**

- Use the real updater plugin under Tauri’s test runtime.
- Serve W11-05’s signed payloads and manifest from an owned loopback server; replace URL origins only in the test copy.
- Require a newer matching-platform release to return the exact verified bytes.
- Require no update for equal/older versions, and refusal for malformed manifests, missing platform entries, wrong keys and tampered downloads.
- Invoke neither installation nor relaunch; keep endpoint/key overrides confined to tests.

**H — CI:** Ubuntu 1–3 minutes; Windows 1–3. **Shares files:** W11-05 and the desktop workflow units; depends on W11-05.

**W11-07 — Inspect actual bundles in a secret-free packaging dry run**

**H — M; medium-high risk, principally bundler-tool provisioning. Value:** detect wrong package names, versions, architectures or payload contents before publication.

**F — Today:** release builds package five desktop targets, while the finalizer verifies uploaded asset metadata and manifest correspondence. Its inventory check requires nonempty assets but does not inspect package contents. MSI/AppImage suffixes are already part of updater discovery. (`.github/workflows/release.yml:59–84`, `.github/workflows/release.yml:118–134`, `.github/workflows/release.yml:229–235`, `scripts/release-gate.py:80–104`, `scripts/generate-updater-manifest.py:16–22`)

**H — Files:**

- Create `scripts/check-bundle-inventory.py`.
- Create `tools/read-msi-inventory.ps1`.
- Create `tools/test_bundle_inventory.py`.
- Edit `.github/workflows/desktop-smoke.yml` and `.github/workflows/release.yml`.

**H — Acceptance:**

- Add a nonpublishing Windows-x64/Ubuntu-x64 packaging mode using `tauri build --debug`, selected MSI/DEB/AppImage targets and W11-05’s disposable signing key.
- Preflight required packaging tools/caches; the acceptance phase must not fetch missing tools.
- Statically inspect MSI tables, DEB metadata/file lists and AppImage contents using provisioned inspection tools.
- Compare package version/architecture, expected application executable and payload paths against the candidate configuration; check updater suffixes and exact signature companions.
- Reject missing executable, wrong version/architecture, empty package and unexpected DB/config/key payloads.
- Run the same checker after real release packaging. Do not replace the completed publication gate.

**H — CI:** Ubuntu 8–15 minutes; Windows 10–20 with warm build/tool caches. **Shares files:** W11-04/05/06 share the dry-run workflow. Actual macOS package inspection remains outside this two-platform unit.

**W11-08 — One public-CLI lifecycle script covering every Stable family**

**H — M; medium risk from distinct RGSS fixture layouts. Value:** give CI and release owners one account-free command proving complete translation-to-restoration behavior.

**F — Today:** Stable covers MV/MZ, XP/VX/VX Ace and Ren’Py. Existing strict CLI roundtrips cover Ren’Py/MV; the independent Ace oracle covers injection/apply/rollback but seeds translations through catalog import. Ren’Py also has a real Mock-provider Add lifecycle. (`docs/COMPATIBILITY.md:20–23`, `crates/cli/tests/ja_en_es_pipeline.rs:891–903`, `tools/test_qa_rgss_oracle.py:333–350`, `tools/test_qa_rgss_oracle.py:395–402`, `crates/cli/tests/add_patch_apply.rs:119–150`, `crates/cli/tests/add_patch_apply.rs:234–244`)

**F — Reusable fixture basis:** RGSS tests already distinguish XP’s 101 text from VX/Ace’s 101 header plus 401 text, and the Python oracle supplies an independent bounded wire-construction vocabulary. (`crates/formats/tests/rpgmaker_vxa_revision.rs:54–67`, `tools/qa_rgss_oracle.py:116–154`)

**H — Files:** create `tools/qa_stable_cli.py` and `tools/stable_cli_fixtures.py`; edit `.github/workflows/ci.yml`.

**H — Acceptance:**

- Accept an explicit already-built CLI path; missing binaries fail.
- Generate six small synthetic cases: MV, MZ, XP, VX, Ace and Ren’Py.
- For each: extract → translate with `mock --no-memory` → validate → Replace inject → strict patch → apply dry-run → apply → rollback.
- Require nonempty extraction, Mock provider attribution, expected translated payloads, unchanged dry-run/source inventories and byte-identical restored payload paths/hashes.
- Record exact CLI SHA, command exits and per-case inventories; never seed translations by SQL.

**H — CI:** Ubuntu 2–4 minutes; Windows 3–6. **Shares files:** W11-01 shares CI; fixture modules remain separate from the existing oracles.

**H — Common execution contract:** reuse checkout `target/`, isolate profiles and fixtures, bound subprocess deadlines, terminate owned processes before removing owned scratch, and upload compact evidence. Follow the repository’s target/cleanup rules and serialize local Cargo work; these constraints are already documented. (`CLAUDE.md:81–106`, `odd/tasks/cycle-2026-11-01.md:95`)

**F — Remaining boundary:** these units cannot establish real-game rendering, native dialog correctness or successful installation/upgrading of an older app. Those remain explicitly separate acceptance requirements. (`CHANGELOG.md:50–54`, `docs/UI-RESPONSIVE-CYCLE.md:71`, `RELEASE.md:115–123`)

**H — Ranked delivery table.** File aliases below expand exactly: `CI` = `.github/workflows/ci.yml`; `DS` = `.github/workflows/desktop-smoke.yml`; `CAP` = `tools/capture_ui.mjs`; `D` = `apps/desktop/src-tauri`.

| Rank | ID | Title | Size | Files edited/created | CI minutes Ubuntu / Windows |
|---:|---|---|:---:|---|---|
| 1 | W11-01 | Gate existing production-browser captures | S | CAP; CI | 1–3 / 1–3 |
| 2 | W11-04 | Native executable startup self-test | M | `D/src/main.rs`; `D/src/self_test.rs`; `tools/qa_desktop_startup.py`; DS | 3–8 / 4–10 |
| 3 | W11-08 | Complete Stable-format CLI lifecycle | M | `tools/qa_stable_cli.py`; `tools/stable_cli_fixtures.py`; CI | 2–4 / 3–6 |
| 4 | W11-02 | Browser authoring and real export | M | CAP | 1–2 / 1–3 |
| 5 | W11-05 | Throwaway signing and manifest verification | M | `scripts/qa-updater-signatures.py`; `D/tests/updater_signature_contract.rs`; `D/Cargo.toml`; `Cargo.lock`; DS | 1–3 / 1–3 |
| 6 | W11-03 | Browser patch/apply/rollback lifecycle | M | CAP | 1–3 / 2–4 |
| 7 | W11-07 | Bundle dry run and static inventory | M | `scripts/check-bundle-inventory.py`; `tools/read-msi-inventory.ps1`; `tools/test_bundle_inventory.py`; DS; `.github/workflows/release.yml` | 8–15 / 10–20 |
| 8 | W11-06 | Real updater check and verified download | M | `D/tests/updater_download_contract.rs`; `D/Cargo.toml`; `Cargo.lock`; DS | 1–3 / 1–3 |

**H — Two disjoint-file lanes:**

| Lane | Suggested sequence | Exclusive file ownership |
|---|---|---|
| A — Browser and CLI | W11-01 → W11-02 → W11-03; W11-08 | CAP, CI, `tools/qa_stable_cli.py`, `tools/stable_cli_fixtures.py` |
| B — Native and release artifacts | W11-04 → W11-05 → W11-06 → W11-07 | DS, desktop Rust files/Cargo metadata, `Cargo.lock`, release workflow, signing and package-inspection scripts |