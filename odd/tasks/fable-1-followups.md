# FABLE-1 follow-ups

Objective: close the ten open follow-ups from the 2026-11-09 Fable project check ([cycle 2026-11-01](cycle-2026-11-01.md), "Follow-ups from FABLE-1"). Branch `fix/fable-1-followups` from `main` 24ecf1e.
Delivery: work-unit commits, push the branch, manual CI (`gh workflow run ci.yml --ref fix/fable-1-followups`), then fast-forward `main`. Runners: `cargo test` per crate, `npm run test:unit` in `apps/desktop`, Python `unittest` for scripts.

## Specs

Source text, verbatim from the cycle ledger: "Follow-ups from FABLE-1 (open, ranked): (1) updater manifest serves MSI/AppImage only while bundle targets "all" also ship NSIS/deb/rpm (owner decision: trim targets or add manifest keys); (2) any http://localhost:<port> origin is trusted, so another local web app can PATCH provider base_url while keeping the stored key (document in RELEASE-SECURITY or narrow origins); (3) synchronous Tauri commands preflight_project_open / get_backups and blocking work in async HTTP handlers; (4) no single-instance guard, two processes can overwrite config.json; (5) HTTP imports capped at axum's 2 MiB default, undocumented; (6) xAI rotated refresh token lost when the token file cannot be written; (7) third-party actions pinned to mutable tags, ci.yml has no top-level permissions; (8) update install has no running-job guard; window.open links may not open in the packaged app; ws.ts reports handler exceptions as parse failures; (9) full cryptographic .sig verification in the publish job; (10) MV with js/rpg_core.js at the root still writes www/data/i18n."

- S1 (item 1): the release ships only installers the updater manifest can update. Decision (reversible, standing grant): trim bundle targets to MSI, app/dmg and AppImage instead of adding unverified installer-specific manifest keys.
- S2 (item 2): the desktop app's backend trusts only the desktop webview origins (plus the Vite dev origin in debug builds). `locust server` keeps loopback-origin trust for browser mode; RELEASE-SECURITY documents both.
- S3 (item 3): `preflight_project_open` and `get_backups` stop running on the Tauri main thread; project open/resume/preflight, catalog import/export and string listing handlers move blocking work off the async runtime.
- S4 (item 4): a second desktop launch focuses the running window instead of starting a second process.
- S5 (item 5): HTTP import endpoints accept bodies up to a documented limit above 2 MiB.
- S6 (item 6): a refreshed xAI token pair survives a failed token-file write for the rest of the process, and the failure is reported.
- S7 (item 7): workflow actions are pinned to full commit SHAs and ci.yml declares read-only top-level permissions.
- S8 (item 8): the update install refuses while a translation or patch job runs; external links open through the shell plugin in the packaged app; ws.ts reports handler exceptions separately from parse failures.
- S9 (item 9): the publish job verifies each updater signature cryptographically against the configured public key before writing latest.json.
- S10 (item 10): Add inject on an MV game with root `js/rpg_core.js` and root `data/` writes `data/i18n/<lang>.json` next to the game's own data.

## Tasks

- T1 | S10 | inline | pending |
- T2 | S8 | inline | pending |
- T3 | S5 | inline | pending |
- T4 | S6 | inline | pending |
- T5 | S3 | inline | pending |
- T6 | S4 | inline | pending |
- T7 | S2 | inline | pending |
- T8 | S1 | inline | pending |
- T9 | S7 | inline | pending |
- T10 | S9 | inline | pending |
- V1 | S1-S10 | independent verifier | pending | Risk items 2 (S2, S6), 4 (S3), 5 (S1, S4, S7, S9).

Route evidence: all tasks inline (no parallel writers: parallel cargo builds froze this machine before, OPS-1/OPS-2). Code map from one read-only explorer on 24ecf1e (path:line per item recorded in L2).

## Log

- L1 (2026-11-24, owner): "haz los pendientess" (after the report that listed the release follow-ups in `odd/tasks/cycle-2026-11-01.md` as the queue).
- L2 (2026-11-24): explorer confirmed all ten items still present on 24ecf1e. Key sites: tauri.conf.json:29-30, generate-updater-manifest.py:16-22, release-gate.py:96; server lib.rs:559-597 origin_allowed; commands.rs:69-81, :974-980; routes/project.rs:100-165, io.rs:292-440, strings.rs:31-250; Cargo.toml (no single-instance); io.rs import handlers take String/Json under axum 0.7's 2 MB default; xai_oauth.rs:485-492; ci.yml no top-level permissions, actions on tags; UpdateChecker.tsx:88-118, ValidationResultsModal.tsx:146, ws.ts:105-146; generate-updater-manifest.py:53-70 shape/key-id only; rpgmaker_mv.rs:2329-2332 hard-codes www/data/i18n.
- L3 (2026-11-24): decisions taken under the standing grant for reversible decisions: S1 trim targets (no manifest keys for installers whose updater support was not verified); S2 narrow origins for the desktop app only, keep browser-mode loopback trust documented.
