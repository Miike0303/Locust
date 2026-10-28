# Review and Editor state consistency

Owner: [overnight UI/API polish, T3 (S1, S3, S4)](../odd/tasks/overnight-ui-api-polish.md).

## Observable behavior

After a successful approval, returning to Editor refreshes the project's cached
string lists, selected detail and statistics. The all-rows table shows the approved
status and saved text; translated/reviewed filters remove the row; the approved
filter includes it. An unchanged repeat approval still advances without another
write. Skip, refused saves and pending saves do not announce a successful change.

Review uses the existing `invalidateProjectData` helper with `refetchType: "none"`.
It marks the captured project's data stale without starting background reads
against a potentially different open database. Editor observers refresh on return.
The shared identity includes database path, game path and format. A replacement
identity resets Review's queue; a late successful save invalidates only its original
identity and cannot advance the replacement queue. Stale approval handlers refuse
to write after the project is cleared or enters a transition. Editor drafts are
not acknowledged or replaced by Review approval, including newer text in a pending
Editor save.

## Deterministic evidence and limits

`Review.test.tsx` invokes the authored component handlers using its existing hook
seam, a real QueryClient, real project/draft stores and new QueryObservers shaped
like Editor's list/detail queries (including the 30-second cache freshness window).
API-boundary fixtures return saved StringEntry values; no backend is launched.

Before the fix, the four navigation/filter checks failed despite the save returning
`approved`. The expanded final test set against the original `03305f6` Review logic
had **43 passing / 9 failing**; with the fix it has **52 passing / 0 failing**.
The same 43 checks remain green, including repeat approval, skip, refusal, pending
actions, context and shortcuts. New checks also cover selected-project cache
isolation, newer pending drafts and late saves across database/game/format changes.
TypeScript checking passes after replacing two unsupported test-only `Array.at`
calls with indexed access.

This proves the bounded cache inconsistency, not browser rendering, HTTP/backend
correctness, native Tauri behavior or game runtime. Candidate-bound browser/API
proof and integrated suites remain parent-owned; historical captures are not RED.
