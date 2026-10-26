# Remaining Rust dependency remediation

The first bounded unit removes **five warnings**: **29 vulnerability records /18 warnings →29 /13**, without suppressing other findings. This is not a clean audit. [B2 S8/D10-D16](../odd/tasks/b2-dependencies.md) owns the remaining work; all work stays local, with no publication or new remote CI claim.

## D11: compatible warning patches — verified locally

| Package | Before | After |
| --- | --- | --- |
| anyhow | 1.0.102 | 1.0.103 |
| event-listener | 5.4.1 | 5.4.2 |
| rand | 0.8.5 | 0.8.6 |
| tokio | 1.50.0 | 1.51.6 |
| chacha20 | 0.10.1 (yanked) | 0.10.2 |
| mio (required Tokio closure) | 1.1.1 | 1.2.0 |
| tokio-macros (required Tokio closure) | 2.6.1 | 2.7.0 |

`Cargo.lock` changes exactly seven package versions and six necessary qualified-reference parent records. The other **769 whole records and lock header remain byte-identical**. Retained official archives, checksums, not-yanked status, declared/documented MSRVs and69 parent requirements admit these versions. This is not minimum-Rust execution proof.

Candidate lock SHA256: `a36db5204794a8fa0a4039540a8d11bab027c38dae458e75176a831615c2026d`.

### Comparable audit and preserved failure

Both actual scans use cargo-audit0.22.2, clean pinned advisory DB `21d909e4c8bd4e14c7aea58bf980c881a0053ab1`, explicit stable Cargo, the same input path and identical private registry knowledge. The baseline actually reports the yanked chacha20 warning. Both scanner children exit1/findings; empty ignore settings remain unchanged.

The first stage (job4) exited1 after3m36s at an overstrict full-object comparison, **after all19 preceding child commands returned expected exits**. Its original100 artifacts remain unchanged. Audit records embed resolved dependency versions: exactly21 Wasmtime findings changed only their direct anyhow dependency reference from1.0.102 to1.0.103. A retained-output postcheck proves each scalar substitution against both lock declarations and resolved metadata; every other field remains identical. No new scan, solve or network request was needed for that correction.

The corrected comparison establishes29→29 vulnerability records,18→13 warnings, exactly five targeted warning removals and **all13 retained warning objects identical**, requiring zero substitutions. Its initial omitted-dependencies serialization assumption for dependency-free number_prefix was corrected once and retained separately; not a product defect.

### Verification

- Parent job5 exit0 after6m53s: frontend1126 passed/0failed/0skipped; production TypeScript/Vite build; Rust2998 passed/0failed/25ignored across120 summaries; fmt and strict locked workspace/all-target Clippy; public collector10/10; lock immutability and classified pre/post cleanup0/freed0.
- Independent verifier `mvpipkov-6-o2nm`: **PASS S3/S8**. Recomputed graph/bytes/archives/constraints, actual raw comparison, scanner/DB1371 tracked files and all49113 private registry files. Parent gates were inspected, not rerun. Read-only Git-object/truncated-output probe limitations were resolved without mutation or product failure.
- The25 ignored Rust tests were not executed. Human `.claude/improvement.md` is excluded and preserved.

**Risk: item5 — dependency update.** Writer self-checks and independent verification are fulfilled; no native/game, broader optional/platform runtime, minimum-toolchain, remote CI or clean-audit claim.

## Remaining family work

| Owner | Next bounded outcome |
| --- | --- |
| D12 | Migrate quick-xml0.36.2→patched0.41.0, preserving XLIFF/TMX references, namespaces, exact errors and atomic rejection. |
| D13 | Migrate optional Wasmtime25→patched36.0.19 with actual feature-enabled tests, not feature removal. |
| D14 | Migrate application ZIP2→patched9; keep updater ZIP4 separate. Latest inspected updater2.14.0 still requires ZIP4. |
| D15 | Maintained mock/progress/font and supported desktop-parent migrations; upstream constraints remain explicit. |
| D16 | Final full-coverage audit, independent results, per-finding remainder table and classified cleanup. |

Inventory evidence establishes that latest supported Tauri/Wry still requires GTK0.18 (glib/proc-macro-error branches), and Wasmtime36 profiling still requires fxprof0.6/fxhash. These are real upstream constraints, not fixes or permission to drop platform/features. No remaining outcome is claimed complete merely from this static map.

## Evidence

Protected parent-owned `.test-tmp/b2-remediation-20261026/`:
- `inventory/REPORT.md`, `FINDINGS.md` and official checksum-verified source/metadata.
- `d11/one-shot/`: baseline/candidate/solver locks, actual metadata, scans, streams, child exits and registry fingerprints; failed stage result retained.
- `d11/postcheck-proof-correction1/`: corrected comparison, exact field witnesses and original-artifact hashes; prior postcheck failure preserved separately.
- `d11/gates/`: full parent check streams and exit files.

The source commit is the work-unit commit containing B2 L51; the observed SHA is recorded in the owning ledger after Git succeeds. Subsequent family units must remain separate review slices rather than publishing the inherited local stack wholesale.
