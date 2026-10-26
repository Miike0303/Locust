# Corrected Home recents: bounded UI verification

## Result

**PASS_BOUNDED**, not native or comprehensive visual acceptance. Local source candidate: `c5cf3c49d5a96965387da067fd45e8f8331d07d9`.

The isolated production-frontend/real-backend run completed all four planned language/viewport cases. The reproduced Home-recents cache defect is fixed without requiring a reload. No live provider or real game was used.

| Case | Reached states / captures | Real HTTP responses | Extracted / approved | Result |
|---|---:|---:|---:|---|
| English, 1440×900 | 18 / 18 | 124 | 2 / 2 | PASS_BOUNDED |
| English, 1024×700 | 18 / 18 | 124 | 2 / 2 | PASS_BOUNDED |
| Spanish, 1440×900 | 18 / 18 | 124 | 2 / 2 | PASS_BOUNDED |
| Spanish, 1024×700 | 18 / 18 | 124 | 2 / 2 | PASS_BOUNDED |
| **Total** | **72 / 72** | **496** | **8 / 8** | **All planned cases reached** |

## Observed behavior

Each case reached Welcome, Editor, Dashboard, Home with the persisted recent, recent reopening, manually translated rows, both review approvals, Check, QA before/after validation, Write, injection cancellation, populated backups, delete confirmation cancellation and referenced-backup deletion refusal.

The four no-write branches compare complete public rows and persisted fixture state, including translations, statuses, metadata, configuration, DB sidecars, game payload, backups and the reference index. Cancellation/refusal checks passed. Each case observed two expected reference-conflict HTTP409 responses; no unexpected request, console or page errors were recorded.

### Reproduction and source correction

- The retained baseline run showed zero recent rows on Home after a ten-second wait, while genuine `GET /api/config` already contained the owned recent. A real reload displayed it. This was a stale 30-second config cache, not missing persistence or a missing selector.
- The source fix dispatches config invalidation after successful shared folder/DB completion without awaiting the optional refresh. Return identity, transport selection, project-query cleanup, refusals and cancellation behavior remain unchanged.
- Focused regression: **15 pass / 5 fail** before the production change, then **20 / 0**. An initial full-build failure from a widened test-fixture type was corrected with an explicit `ProjectOpenResponse` annotation; failed logs remain retained.
- Final frontend: **1126 / 0**, production build passes. Fresh-asset Rust/Tauri integration: **2998 / 0 / 25 ignored**, 120 suites; fmt, strict Clippy and classified cleanups pass.
- Independent verification: **20 copied tests + 14 public-flow probes**, including real `QueryClient`/`QueryObserver`, pending refreshes and refusal/abort behavior. The verifier corrected one scratch-probe escaping error before assertions; that is not product RED evidence. Parent final-source focused spotcheck passed.

## Candidate and evidence binding

Evidence root: `.test-tmp/continuation-20261025-evidence/ui-acceptance/` (retained locally, not committed binaries or datasets).

- Successful run: `run-20261025-221952-1792988392807229900/report.json`; job27 exited0 after39 seconds.
- New immutable input: `production-auto12-c5cf3c4/`, **29 verified artifacts**.
- Provenance SHA256: `e2847bcea5939d661828307c13cd86aa69a803c2e9f2626e0235fd43b2d5cb38`.
- Runner: `acceptance-auto12.py`, SHA256 `b24f0f863ce2527422ce7694ce213bf5f9d6cbc0003968f7d4da3bf3c851d2cc`.
- Only three frozen-input bindings differ from the retained diagnostic runner; the remaining normalized AST is identical. No network, process-ownership or cleanup guard changed.
- Frontend: job25 Vite production build from the same source bytes committed as `c5cf3c4`.
- Backend: the original job11 **debug CLI**, explicitly retaining backend reference `d0f40dc5692dc2fe785c522e76b4a412b3129e3d`, not a new release executable. Cargo/crate differences through `c5cf3c4` contain only two integration-test paths; production backend source is unchanged.
- Original frozen artifacts and all failed-run traces/captures remain intact. Source and frozen-candidate hashes were unchanged across the successful run.

## Cleanup evidence and limits

All four cases observed normal SDK context-manager exit, context closure, browser disconnection, exact owned-backend `Popen.wait`, empty backend listeners, closed static sockets and terminated static threads. Scoped cleanup verdict: **PASS_SCOPED**. Backend exit1 followed controlled Windows termination; it was not an application-test failure.

The following are **not verified or accepted** by this run:

- Whole-SDK OS descendant closure: **UNVERIFIED**; no discovery or forced termination of unrelated processes was attempted.
- Unverified-legacy-backup warning: **UNAVAILABLE** through the authorized public fixture setup; no metadata or private storage was fabricated.
- Native Tauri, human first-minute experience, live providers, real games, game runtime, fonts or overall visual correctness.
- Readability of every transient notification. Parent spotchecked Spanish narrow Home and English wide backup refusal. Home displays its recent correctly. The refusal capture has a partially off-edge/faded toast; that image alone is not readable-error acceptance, although the exact DOM message, genuine409 and unchanged state were verified.

Owner ledger: [autonomous continuation](../odd/tasks/autonomous-2026-10-25.md), AUTO6/AUTO12. The remaining dependency prerequisite/recovery work is separate; this report does not declare the autonomous product goal or quota consumption complete.

Risk: item3 — shared project-open completion behavior; independent source verification passed. Existing runner-isolation review and runtime limits remain explicit above.
