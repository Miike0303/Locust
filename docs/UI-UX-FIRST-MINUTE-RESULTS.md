# UI/UX first-minute results

## Outcome

The six scoped UI slices and follow-up cleanup are implemented and locally committed. Synthetic web acceptance supports the visual and cold-load goals. Human onboarding, first provider translation, native Tauri and real-game behavior are not certified.

Source candidate: `69f2236` on `feat/ui-ux-first-minute`. Nothing was published. See [owner ledger](../odd/tasks/ui-ux-first-minute.md) and [Claude audit](UI-UX-FULL-AUDIT.md).

## Evidence

Both retained roots have `.keep`, metrics, CDP/network logs, asset sizes and the same 36 paired PNG/DOM snapshots:

- Before: `C:/Users/Mike/AppData/Local/Temp/locust-owned-ux1-_yhlv87h` (job-26).
- After: `C:/Users/Mike/AppData/Local/Temp/locust-owned-ux1-ontao9xy` (job-38).

Production frontend, isolated real loopback backend, disposable Chrome profile, 1440×1000 viewport and synthetic 200/2000-row Ren'Py fixtures. The routing shim only redirects the backend destination; responses are real. Each capture reports 8/8 helper checks: three input-existence checks, empty recents, no project before onboarding, extracted row counts and no uncaught runtime exceptions. These are not eight comprehensive workflow tests.

Final parent frontend suite: **722 passed, 0 failed, 0 skipped**; production build exit 0. Existing mixed static/dynamic API-import and large-chunk warnings remain. Rust was not rerun for these frontend-only slices.

## Measured cold Home

Use the first phase's request count and first `Network.requestWillBeSent` events, not whole-session totals.

| Metric | Before | Final |
|---|---:|---:|
| Requests | 19 | 14 |
| GET / OPTIONS | 14 / 5 | 10 / 4 |
| Config GET | 2 | 1 |
| Patch/Recovery modal fetches | 2 | 0 |
| favicon.ico request | 1 (404) | 0 |
| Loaded JS files | 6 | 4 |
| Loaded JS raw bytes | 592,666 | 551,285 |
| Computed per-file gzip sum | 173,981 | 163,434 |
| Loaded CSS raw bytes | 46,100 | 40,681 |

Final JS: index 528,532 + Welcome 20,951 + plus 861 + shield 941 bytes. Reduction: **41,381 raw bytes**. The intermediate slice-4 value 552,137 is not the final measurement. Gzip is computed metadata; the test server served uncompressed assets. Cold goals (≤18 requests, config once, no modal chunks) pass.

| Automated extraction timing | Before | Final |
|---|---:|---:|
| Browser navigation → strings visible | 1,404.6 ms | 1,137.0 ms |
| Open action → strings visible | 428.0 ms | 432.2 ms |
| Primary actions | 2 | 2 |

Single runs only: no statistical speedup or variance claim. Post-open time was slightly higher in the final observation. This is extraction, not a human first-minute trial or a provider-produced translation.

## Visual acceptance readback

- Home 01/03: one solid Open action, secondary single-file action, collapsed expert entry points and full formats; stable engines inline. No registration or welcome carousel added.
- Six no-project routes: one “Go to Home” action each. Actual Speakers image places it immediately below its description. Exact ≤120 px spacing is unmeasured because snapshots contain no bounding rectangles.
- Editor 04/11: toolbar Translate solid; workflow action outlined. Final 11 shows hollow accent translated dots and accent progress segment.
- Translation modal 07: “Google Translate (free)” appears once.
- Inject 08: all language rows visible at the captured viewport, summary “Español”; warning explicitly describes last-loaded counts, not guaranteed current counts or write safety.
- Patch 09: Rollback outlined while Apply appears disabled. No patch operation executed.
- Returning Home 05 DOM and 14 ES image: Continue project is primary. Not every individual image was independently reread in final comparison.
- Appearance 13 ES: interface language first; no visible clipping in inspected Appearance/Home images. No numerical overflow or responsive/short-height certification.

## Requirement disposition

| Spec | Result and limit |
|---|---|
| S1 | Captured web UI improved with Claude design audit and scoped implementation; no native certification. |
| S2 | Request/cache and lazy modal improvements measured. No React causal render profiling, SQL/N+1 profiling or image/font optimization claim. |
| S3 | Chrome/CDP evidence retained. Playwright/MCP/Sentry/production monitoring not executed or required by this optional suggestion. |
| S4 | Reduced first-screen cognitive load; abandonment/retention unmeasured. |
| S5 | One dominant action, no registration/carousel, synthetic extraction under one minute. First translated result unmeasured. |
| S6 | Hurried-newcomer critique delivered; moderated human trial missing. Remaining likely abandonment: user arrives without a game ready and cannot see demonstrated translation value. |

No slow endpoint/N+1 symptom was demonstrated at the tested scale. Endpoint latency includes transport/serialization/database work, not SQL attribution. No speculative optimization was applied.

## Preserved behavior and local slices

| Slice | Commit |
|---|---|
| Empty-state contract | `cacb508` |
| Home single decision / Continue | `150201f` |
| Editor primary / provider label / status dots | `85aa194` |
| Shared boot config / lazy modals / favicon | `fba2919` |
| Inject guidance / Rollback hierarchy | `c22da61` |
| Settings language / configured-default indicator | `ad7f280` |
| Retired guide cleanup / progress accent | `69f2236` |

All expert entry points, experimental badges, project/queue flows and patch confirmations/guards were retained. No backend language defaults, provider calls, game writes, API payloads or injection/patch safety logic were changed. No stored preferences were deleted by retired-guide cleanup. Local commits are separate review units; inherited history since origin/main is not this feature's delivery scope.

## Failed and omitted proof

The ledger retains failed baseline-helper runs, old-label recapture failure, worker test-harness/type failures, the slice-5 copy expectation failure (698 pass / 2 fail, subsequently corrected) and the initial assessment stop on an untracked test (resolved by declaring/staging the intended test). Final checks pass; none of those failures were erased or recast as success.

Not executed: live provider translation, real-game injection/apply/rollback, native Tauri, human onboarding, accessibility certification, React render attribution, SQL profiling, production telemetry. Cleanup used conservative age/prefix bounds to preserve evidence and reusable helpers. The nonfatal Bash missing-/tmp warning remains; no global environment repair was made.

## Needs your decision

To claim human first-minute completion, arrange one moderated, timed cold-profile trial with a real newcomer, recording **first extracted strings separately from first translated string**. Real-provider use and native/game QA remain separate scope/authorization boundaries. The engineering implementation and synthetic acceptance report are complete; these missing proofs are not silently certified.

Risk: none for this report (passive documentation). Runtime slice risks and checks are recorded in the owner ledger.
