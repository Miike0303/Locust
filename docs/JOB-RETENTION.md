# Job WebSocket retention

Previously each job retained every JSON event until cleanup, each connection
cloned that entire history, and a connection kept serialized event strings
forever to suppress duplicates. A broadcast overrun only logged a warning.

`JOB_REPLAY_LIMIT = 256` now bounds retained frames per job, including snapshots
and the terminal. Consecutive `batch_completed` frames, or consecutive patch
`progress` frames, replace the previous frame. At capacity, discard the oldest
unpinned frame. Always retain the initial non-progress/start frame, the newest frame, and the latest
of each state type: `started`, `batch_completed`, `progress`, `string_translated`,
`provider_switched`, `validation_failed`, `paused`, `resumed`, and `batch_failed`. Replay stays
in publication order. An initial patch progress frame also coalesces into latest
progress. This preserves initial/current totals, cost/completeness,
preview, provider, validation, pause/resume and latest failure state despite informational bursts.

All other informational events (including older `batch_failed` diagnostics)
remain until the cap requires eviction. At most eleven frames are pinned; the
remaining slots hold the most recent unpinned history that fits. Older diagnostics
and row history can therefore be absent on a fresh reconnect or after an overrun;
historical warning counts/logs are limited to retained events. Authoritative
progress and terminal totals are unchanged. Ordinary live delivery is not coalesced
or capped by replay, and distinct equal-payload publications are delivered separately.

Every production publication receives a monotonically increasing per-job `seq`
under the same mutex as compaction and broadcast. The first terminal (`completed`,
`failed`, `done`, or `error`) wins, remains the final frame, and closes the socket;
later worker/cancellation events are rejected. Job cleanup timing is unchanged.

Subscribe before taking the compact snapshot. Each connection retains one `u64`
watermark, skips `seq <= last_sent`, and on broadcast `Lagged` rereads compact
replay. Holding its replay `Arc` also permits recovery after map cleanup. Thus
subscribe/snapshot overlap and lag recovery do not duplicate delivered frames;
only intentional replay compaction/retention can omit historical frames.

Translation's additive `seq` field is compatible with desktop consumers: `lib/ws.ts` parses
JSON and dispatches on `type`; handlers read named fields and accept unknown ones.
`lib/translationJobSession.ts` dedupes serialized payloads across reconnects, so
stable sequence identity also distinguishes new identical diagnostics from replay.
Patch sequences remain internal and are removed before sending JSON. No frontend
changes are required.

The bound is on frame count, not arbitrary payload bytes: strings, validation
issues and terminal reports retain their original contents. Vector capacity can
reach 512 slots, with at most 257 frames transiently during insertion; each
connection's replay copy holds at most 256 frames. Broadcast capacity remains
1,000 frames (Tokio rounds its ring allocation to a power of two).

The regression fixture with one start and 100,000 batch progress publications
measures **100,001 frames / 11,788,948 serialized bytes before**, versus
**2 frames / 193 serialized bytes after**, including sequence fields. Serialized
bytes measure replay JSON, not allocator/heap overhead.
