# Patch compression

`locust patch` supports three opt-in ZIP policies. The policy applies to payloads,
`locust-patch.json` and `README.txt`; it does not change the patch schema.

| Policy | ZIP writer | Tradeoff |
| --- | --- | --- |
| `default` | Deflated with the existing ZIP library defaults | Existing behavior and compression level |
| `fast` | Deflated, level 1 | Less compression work, potentially larger archives |
| `store` | Stored, without compression | Avoids compression work; approximately the payload size |

## Entry points

```powershell
locust patch C:\synthetic\recorded -P C:\synthetic\project.db -o C:\synthetic\patch.zip --compression fast
locust patch C:\synthetic\recorded -P C:\synthetic\project.db -o C:\synthetic\patch.zip --compression store
```

Omitting `--compression` is equivalent to `--compression default`. Unknown values
are rejected during CLI parsing, before opening the project or creating output.

`POST /api/patch/pack` accepts an optional `compression` string:

```json
{
  "game_path": "C:/synthetic/recorded",
  "output_path": "C:/synthetic/patch.zip",
  "languages": ["es"],
  "pristine": true,
  "pristine_path": "C:/synthetic/pristine",
  "compression": "fast"
}
```

Omitted HTTP fields use `default`. Unknown strings, `null` and non-string values
are rejected by JSON extraction (HTTP 422), before packing. Existing response
fields and manifests are unchanged.

The desktop **Patch → Pack** tab has an EN/ES compression select and a size/speed
hint. It remembers the opt-in preference under `locust.patch.compression`; a
fresh profile uses `default`. An explicit `patchPack({ compression: ... })`
overrides the desktop preference. If storage is unavailable, a changed selection
is retained for the session. Compression does not affect apply or rollback UI.

Existing Rust `PackOptions` literals and calls continue to work. Opt in with:

```rust,ignore
pack_injection_recording(&db, options.with_compression(PatchCompression::Fast))?;
```

The same builder works with `pack_with_pristine_backup` and
`pack_recorded_generation`. `PackConfiguration` wraps the original options so
adding a compression policy does not require changes to existing callers.

## Compatibility and safety evidence

The `default` branch constructs the exact previous options:
`SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)`.
It does not set an explicit compression level, timestamps, permissions or flags.

`patch_compression_implicit_and_explicit_defaults_match_legacy_zip_bytes` packs
both existing `PackOptions` and an explicit `Default` configuration. Each archive
is reconstructed with the former writer options and the exact emitted entry
bytes/timestamps, and compared byte for byte, including headers and the central
directory. UUIDs and creation times naturally differ between independent packs;
the test preserves them instead of normalizing the comparison.

All policies keep one serial writer and the existing bounded streaming hash of
the exact payload bytes written. No extra payload buffering or hash pass was
added. Existing game locks, backup selection, atomic publication, original hashes
and path checks stay on the same code path. ZIP64 remains enabled for entries
at or above 4 GiB.

New regression coverage checks all three policies for payload hashes, strict
verification, replacement/addition, apply, rollback, output containment, same-size
recording drift, and corrupted ZIP payloads refused before game writes. A small
forced-ZIP64 fixture tests the streaming header/readback for all policies; it is
not a physical 4 GiB file test. HTTP coverage exercises omitted/default/fast/store
requests and invalid values. Desktop tests exercise EN/ES labels, the actual
select and Pack callbacks, request serialization, explicit overrides and storage
failure. Existing modal tests also run unchanged.

The policy lifecycle test also compares each complete ZIP with an independent
writer configured for that policy, including fast's level 1 Deflate stream.

## Synthetic measurements

The reproducible benchmark is in `crates/core/tests/patch_compression.rs`:

```powershell
cargo test --release -p locust-core --test patch_compression patch_compression_measurements -- --ignored --exact --nocapture
```

It uses only newly generated, operation-owned temporary directories; no game
data. Cargo's existing `CARGO_TARGET_DIR` is inherited unchanged. Fixtures are:

- **Text:** 64 files × 1 MiB (64 MiB), UTF-8 dialogue-like lines with varying
  scene/line/speaker/object/room identifiers.
- **Container:** 2 files × 64 MiB (128 MiB), each block containing 6.25% repeated
  metadata and 93.75% deterministic high-entropy synthetic asset bytes. These
  model opaque containers dominated by compressed resources; they are not an
  actual engine format or a claim about real-game performance.

Each file has a distinct pristine original and a recorded SHA-256. Every trial
runs pack and apply in separate fresh test processes. Three trials rotate policy
order (`default/fast/store`, `fast/store/default`, `store/default/fast`). Fixture
creation, recording, database open, installation-copy setup and rollback are
outside the timed regions. Pack includes locking, pristine hashing, streamed
payload hashing/compression and ZIP publication. Apply includes verification,
staging, backup, writing and receipt creation, using strict hashes without legacy
confirmation. Installed hashes and rollback are checked after timing.

RSS is the process high-water resident working set, captured separately for pack
and apply with Windows `K32GetProcessMemoryInfo` (Linux uses `VmHWM`). It includes
process/test-harness overhead; apply RSS is captured before post-apply test checks
and rollback. Disk caches are not flushed. These are local warm-cache synthetic
measurements, not guarantees for another CPU or storage device.

Measurement host: Windows build 26100, AMD Ryzen 7 7700X (16 logical processors),
Rust/Cargo 1.94.0, Node 24.17.0. Results below use optimized release code.

Median of three trials per row; RSS is the median of each process peak.
There were 18 trial/policy pairs and 36 separate timed operation processes.

| Recording | Policy | Pack (ms) | Apply (ms) | ZIP (bytes) | Pack peak RSS (MiB) | Apply peak RSS (MiB) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Text, 64 MiB | `default` | 2005.1 | 2775.1 | 6,781,185 | 14.14 | 14.12 |
| Text, 64 MiB | `fast` | 377.6 | 3294.2 | 9,440,737 | 14.16 | 14.12 |
| Text, 64 MiB | `store` | 285.9 | 2706.7 | 67,132,857 | 13.77 | 14.12 |
| Container, 128 MiB | `default` | 5231.3 | 1357.9 | 125,922,387 | 14.06 | 13.93 |
| Container, 128 MiB | `fast` | 1347.3 | 1575.1 | 126,057,406 | 14.05 | 13.93 |
| Container, 128 MiB | `store` | 319.0 | 1169.8 | 134,219,321 | 13.66 | 13.91 |

Fast reduced median pack time by 5.31× for text and 3.88× for containers,
with ZIP growth of 39.2% and 0.11%, respectively. Its apply medians were 18.7%
and 16.0% slower. Stored archives were largest and had the lowest observed pack
and apply medians in both fixtures. These comparisons cover these synthetic
recordings and this host.

The parent benchmark and all 36 operation subprocesses passed. Installed hashes
and restored original hashes were checked after each apply. Local raw trials,
medians and gate logs are retained under `$env:CARGO_TARGET_DIR/w3-05-results/`
(`benchmark.log`, `measurements.json`, `checks.json` and individual check logs).

## Checks

| Check | Passed | Failed | Ignored | Result |
| --- | ---: | ---: | ---: | --- |
| `cargo test -p locust-core patch` | 108 | 0 | 2 | PASS; the two opt-in benchmark helpers are ignored here |
| `cargo test -p locust-cli --no-fail-fast` | 241 | 3 | 0 | Sandbox hard-link failures below |
| `cargo test -p locust-server --no-fail-fast` | 276 | 1 | 2 | Sandbox hard-link failure below |
| `cargo fmt --all -- --check` | — | — | — | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | — | — | — | PASS |
| `npm --prefix apps/desktop run test:unit` | 1272 | 0 | 0 | PASS |
| `npm --prefix apps/desktop run build` | — | — | — | PASS |

All ten new non-benchmark regression tests pass. Existing frontend dependencies
were present, so `npm ci` was not needed. CLI and server were run with
`--no-fail-fast` to obtain complete counts despite environment failures; the
requested plain CLI command also encountered the first backup hard-link failure.

Sandbox-only failures, by name:

1. `cli_backups_total_counts_shared_payload_once` — backup reuse falls back to
   copying, so two 8-byte originals consume 16 bytes rather than 8 physical bytes.
2. `cli_export_refuses_sidecars_and_hard_links_even_with_overwrite` — hard-link
   fixture creation returns Windows error 5 / `PermissionDenied`.
3. `tmx_export_filters_overwrite_and_database_sidecar_guards_are_shared` —
   hard-link fixture creation returns Windows error 5 / `PermissionDenied`.
4. `routes::backups::backup_operation_tests::backup_http_delete_preserves_other_shared_backup`
   — the same physical-size expectation (16 versus 8 bytes) after copy fallback.

A separate disposable hard-link probe in the checkout's test scratch directory
also returned Windows error 5. These tests and the backup/export implementation
were not changed. No junction or local-data-directory failure occurred in these
requested checks. The patch/hash/apply/rollback/compression tests all passed.

## Changed files

Added/deleted source lines relative to `a84b56d`; new files count all lines.

| File | Added | Deleted |
| --- | ---: | ---: |
| [apps/desktop/package.json](../apps/desktop/package.json) | 1 | 1 |
| [apps/desktop/src/components/patch/PatchPackTab.test.tsx](../apps/desktop/src/components/patch/PatchPackTab.test.tsx) | 109 | 0 |
| [apps/desktop/src/components/patch/PatchPackTab.tsx](../apps/desktop/src/components/patch/PatchPackTab.tsx) | 36 | 1 |
| [apps/desktop/src/lib/api.ts](../apps/desktop/src/lib/api.ts) | 25 | 1 |
| [apps/desktop/src/lib/i18n/en.ts](../apps/desktop/src/lib/i18n/en.ts) | 5 | 0 |
| [apps/desktop/src/lib/i18n/es.ts](../apps/desktop/src/lib/i18n/es.ts) | 5 | 0 |
| [crates/cli/src/commands/patch.rs](../crates/cli/src/commands/patch.rs) | 65 | 1 |
| [crates/cli/src/main.rs](../crates/cli/src/main.rs) | 14 | 1 |
| [crates/core/src/patch/mod.rs](../crates/core/src/patch/mod.rs) | 1 | 1 |
| [crates/core/src/patch/pack.rs](../crates/core/src/patch/pack.rs) | 130 | 13 |
| [crates/core/tests/patch_compression.rs](../crates/core/tests/patch_compression.rs) | 429 | 0 |
| [crates/server/src/routes/patch.rs](../crates/server/src/routes/patch.rs) | 103 | 1 |
| [docs/PATCH-COMPRESSION.md](../docs/PATCH-COMPRESSION.md) | 198 | 0 |

**13 files: +1121 / −20 lines.**
