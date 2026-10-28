# Editor edit refresh measurements

Measured on 2026-10-28 for NEXT-05. The four cases issue **43 requests before and
10 after**, a **76.7% reduction**. Intended user-visible behavior is unchanged.

## Method

`apps/desktop/src/lib/editInvalidation.test.ts` invokes the authored event
handlers in `DetailPanel` and the table's inline editor, using the refresh
callback produced by the real `Editor` page. Draft operations, `patchString`,
translation-write tracking, project gates, QueryClient and QueryObservers are
real. React hooks are controlled without a DOM; Tauri IPC and embedded HTTP
responses are mocked. Server responses include the complete saved row.

Each fixture has six rows and a two-row page. The active read observers are the
current strings page, stats, selected detail, validation summary, expanded
row-0 history, and file/tag facets. Dashboard, other-row histories and memory
caches are inactive. Initial data is preloaded, so counts exclude mounting,
PATCH requests, session-only QA timestamps, and unrelated startup work. This
measures request starts, not elapsed time or backend SQL work.

Cases (a) and (b) save a translation and approve a row, respectively. Case (c)
saves an empty translation while searching for `Before`, removing the edited
row and requiring the server to refill the page and correct its total. Case
(d) completes five inline saves sequentially while earlier reads are held on a
network latch; row-0 detail/history stay open. Reads capture a snapshot before
the latch, so simply joining an old request cannot satisfy the final-state
assertions. Separate status-race tests verify final counts change correctly.

Before changing production code, all four request-budget tests failed, while
their assertions for page contents/totals, stats, detail, history and QA evidence
passed. The original rapid case also started overlapping reads for every key.
The final harness checks exact family counts, duplicates, overlap, and the
aggregate reduction of at least 30%.

Run the focused harness from `apps/desktop`:

```powershell
.\node_modules\.bin\tsx.cmd --test src/lib/editInvalidation.test.ts
```

## Request starts, before → after

| Case | Strings page | Stats | Detail row | Row history | Validation summary | Total | Repeated key requests | Overlapping same-key requests |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| (a) Save translation | 1 → 0 | 1 → 1 | 1 → 0 | 2 → 1 | 1 → 0 | **6 → 2** | 1 → 0 | 1 → 0 |
| (b) Change status | 1 → 0 | 1 → 1 | 1 → 0 | 2 → 1 | 1 → 0 | **6 → 2** | 1 → 0 | 1 → 0 |
| (c) Save removes filtered row | 1 → 1 | 1 → 1 | 1 → 0 | 2 → 1 | 1 → 0 | **6 → 3** | 1 → 0 | 1 → 0 |
| (d) Five rapid saves | 5 → 0 | 5 → 2 | 5 → 0 | 5 → 1 | 5 → 0 | **25 → 3** | 20 → 1 | 20 → 0 |
| Combined | 8 → 1 | 8 → 5 | 8 → 0 | 11 → 4 | 8 → 0 | **43 → 10** | 23 → 1 | 23 → 0 |

Repeated key requests are starts beyond the first start for each complete
QueryClient key within a case, including sequential requests. The remaining
repeat is a necessary serial stats catch-up after later writes, rather than
an overlapping request. Facets, file stats, validation issues, current-project,
translation runs, injection state, memory and config each issue **0 → 0** reads
in these four fixtures. The original broad helper already excluded facets and
memory; these measurements do not claim savings from them.

## What changed

- Successful row writes pass the server row and its previous value to Editor.
  `refreshEditedString` replaces the detail and matching cached page rows with
  `setQueryData`. Pre-save reads of those entries are cancelled so late results
  cannot restore stale values.
- Status/file membership changes stale affected pages across offsets. Active
  pages refetch for authoritative totals and pagination. Search and tag filters
  are handled conservatively on the server because their SQL LIKE semantics
  include wildcards. Unrelated projects/statuses/files remain untouched.
- Only stats/file counts and the edited row's history are invalidated. Closed
  histories and other inactive affected caches stay stale until observed.
  File/tag facets refresh only if the returned file path or tags change; the
  current facets contain no status counts.
- Per-client, per-query scheduling joins concurrent reads with
  `cancelRefetch: false` and performs a trailing read when writes arrive after a
  snapshot began. It checks the project gate again before every read.
- A status action following a dirty save uses that save's returned status as
  the previous value, preserving both transitions when computing page changes.

## What was intentionally kept

- Broad refreshes for bulk operations, translation completion and explicit
  conflict “Load latest” remain in place. Existing broad-invalidation tests
  were preserved without changes or weaker assertions.
- QA issue/summary data remains the persisted last validation snapshot.
  `patchString` still records translation staleness through `workflow-last-edit`
  in the original project's cache, including same-millisecond writes. Status
  alone leaves QA current, as existing tests require.
- Project-switch cancellation, reconnect guards, scoped draft identities,
  optimistic translation guards, history restore and explicit overwrite are
  preserved. Delayed saves cannot refresh another database's queries.
- Selection remains available in detail after its row leaves the table filter.
  No Rust code, API contracts, draft persistence or user workflow was changed.

## Validation

`npm --prefix apps/desktop ci`, `npm --prefix apps/desktop run test:unit`, and
`npm --prefix apps/desktop run build`. The new harness has 15 tests, covering the
four measured cases, aggregate reduction, filtered pagination, stale reads,
trailing counts, two-write status changes, project switches, conflicts,
closed history and facet changes.

All **1,211** unit tests pass (**1,196** existing and **15** new), and the
production build passes. Existing frontend test files were not modified.
