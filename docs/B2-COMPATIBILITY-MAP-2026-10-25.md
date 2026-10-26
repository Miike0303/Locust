# Dependency compatibility follow-up map

Read-only follow-up for [AUTO-5](../odd/tasks/autonomous-2026-10-25.md). This maps locked dependencies, cached manifests and existing compatibility tests. No dependency was changed, built, scanned or downloaded for this research. **No vulnerability is claimed fixed.**

## Recommended next unit

Once authoritative advisory ranges, a comparable scanner/database and an approved target source are available, isolate **core's quick-xml migration** as one compatibility unit. Limit changes to `crates/core/Cargo.toml`, the necessary XML lock closure, `export.rs`, `export/tmx.rs` and directly relevant existing XML tests. Reject unrelated dependency churn. Stop if that surface cannot contain the migration.

The recorded project-owned scanner and advisory database are absent; see AUTO-4 in the owning ledger. Historical counts of 30 records, 27 IDs and 17 warnings are not a fresh baseline. Cached newer versions are not evidence that those versions fix the reported advisories.

## Locked families and activation

| Family | Locked versions | Activation and separation |
| --- | --- | --- |
| quick-xml | 0.36.2, 0.41.0 | Core directly requests 0.36 with `serialize`; ordinary runtime dependency. The 0.41.0 duplicate comes through plist/Tauri macOS tooling/runtime chains, not a replacement for core's parser. |
| zip | 2.4.2, 4.6.1 | Core/formats/CLI request ZIP 2 with default features disabled and `deflate` enabled; application patch/font/NW paths use this family. ZIP 4 is a separate Windows Tauri updater chain. CLI's inspected direct `zip::` uses are tests; production packing resides in core. |
| wasmtime | 25.0.3 | Optional `locust-core/wasm-plugins`; core defaults are empty and no member enables the feature by default. Explicit activation includes parallel compilation/Rayon. Lock presence does not establish advisory-trigger execution. |

Source evidence: `Cargo.lock` records at 4454/4464, 8371/8388 and 7123; `crates/core/Cargo.toml:27-32,49-51`; `crates/formats/Cargo.toml:23`; `crates/cli/Cargo.toml:27`; `apps/desktop/src-tauri/Cargo.toml:25` and `src/main.rs:55`. Line numbers describe the inspected continuation and may drift.

Cached Cargo manifest declarations: quick-xml 0.36.2 Rust1.56, 0.41.0 Rust1.79; Wasmtime25.0.3 Rust1.78; ZIP2.4.2 Rust1.73, ZIP4.6.1 Rust1.82. These are declarations, not a tested minimum-toolchain claim or proposed fixed versions.

## Compatibility contracts to retain

### XML catalogs

- Namespace/entity/CDATA/whitespace handling, line endings versus character references, complete-document rejection and exact entry identity.
- Restricted bilingual TMX profile, distinct explicit languages, review markers, encoded IDs and existing document/unit/field limits.
- Parse-before-mutation and transactional import; exact public error/exit behavior and unchanged database on refusal.

Call sites: `crates/core/src/export.rs:370`, `export/tmx.rs:185`; CLI parsing in `crates/cli/src/commands/io.rs:202-241`, batch transaction in `crates/core/src/database/strings.rs:558-589`.

Existing proof surfaces: core `tmx_catalog_tests`, `stale_catalog_roundtrip`, XLIFF module tests; CLI `tmx_cli_tests` and `xliff_import_tests`. Future command forms, not executed by this research:

```text
cargo test --locked -p locust-core --test tmx_catalog_tests
cargo test --locked -p locust-cli --test tmx_cli_tests --test xliff_import_tests
```

### ZIP packaging and recovery

Retain recording-selected payloads, manifest/pristine hashes and sizes, addition identities, verification before mutation, byte-exact rollback and recovery authority. Preserve traversal/drive/ADS/device/link refusal, case collisions, duplicate raw headers and declared/actual streaming bounds. NW archives also require untouched members and executable-prefix preservation.

Existing proof surfaces: core `patch_apply`, `patch_plan_integrity`, `patch/pack.rs` tests; CLI `add_patch_apply`; formats `tyrano_nw` and `decompression_bounds`. ZIP's migration footprint spans pack/verify/release-entry, font archives and NW reconstruction, not merely writer options.

### Optional plugins

Existing Wasmtime code covers directory selection, invalid-module skipping, metadata/memory/allocator interfaces and typed extraction/injection functions. Inspect `crates/core/src/wasm_plugin.rs` before a separate runtime migration. The valid example test is ignored, and no fuel/epoch deadline is configured at `Engine::default()`; bounded startup time and valid-plugin execution are **not established guarantees**. No production loader caller outside the module was found in this bounded inspection.

## Proof limits and next action

This static map does not establish exploitability, fixed advisory ranges, target API compatibility, native/runtime coverage or minimum-toolchain behavior. No implicit scanner installation, major upgrade or blanket Cargo update is authorized by the map. Restore the missing audit prerequisites before selecting a dependency edit; unrelated autonomous queue work can continue.

Raw research with precise cached-manifest paths is retained in `.test-tmp/continuation-20261025-evidence/dependency-reachability-research-result.md`; that evidence is protected from generic scratch cleanup.
