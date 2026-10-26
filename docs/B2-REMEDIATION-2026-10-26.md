# Remaining Rust dependency remediation

Two bounded units now remove **five warnings and two XML vulnerability records**: **29 vulnerability records /18 warnings →27 /13**, without suppressing other findings. This is not a clean audit. [B2 S8/D10-D16](../odd/tasks/b2-dependencies.md) owns the remaining work; all work stays local, with no publication or new remote CI claim.

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
| D12 | Complete locally: quick-xml0.41.0, exact public compatibility and independent S3/S8 PASS; two findings removed. |
| D13 | Complete locally: Wasmtime36.0.19, actual optional contracts and full gates/independent S3/S8 PASS; audit6/12. |
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


## D12: XML migration — verified locally

The core manifest now pins quick-xml `=0.41.0` with `serialize`. The minimal Cargo-generated closure removes0.36.2, adds the existing serde edge to0.41.0 and requalifies only core/plist XML references; **778 other whole records and the header remain exact**. Isolated job8 exited0: **29/13→27/13**, removing only the two XML vulnerabilities, with every remaining full vulnerability/warning object and setting identical (zero field transforms).

The API migration coalesces Text/GeneralRef runs for one existing unescape operation, preserving whole-run error ranges/precedence, literal entity single expansion, CDATA separation, attribute whitespace and the distinct existing XLIFF/TMX character-admission policies. Public diagnostics are mapped only where dependency lexer wording split. No CLI flag or requested contract changes.

Actual old-parser baseline: core export45, TMX15, CLI11+4 and31 public CLI cases passed. Actual candidate: core export47, TMX16, CLI11+4 and all31 cases passed; exact exit/stdout/stderr, catalog bytes, status-filtered public projections, refusal/no-write and destination-preservation comparison has **zero differences**. Frontend1126/0/build passed. After formatting-only correction, job11 exited0: Rust3005/0/25ignored across120summaries, fmt, strict locked workspace/all-target Clippy, collector10/10, lock/source502hash immutability and classified cleanup. The ignored tests were not run.

Preserved failures are separate from product defects: job9 stopped before compilation on the exact authorized L57 ledger append; a pinned780-byte reconciliation retained all source/human checks and586 original artifacts. Job10 passed focused/differential/frontend checks, then failed fmt before Clippy/workspace; rustfmt changed only wrapping/optional trailing commas on three known files. The parent's initial whitespace-only assertion rejected those commas and was retained. The textual `format-correction/before` copy uses Windows CRLF due to read_text/write_text capture; it is **not byte-authoritative**. Actual staged raw source/gate hashes match, and normalized captured text is identical. Final after-snapshots use raw bytes.

Independent verifier `mvpl78xx-a-ai11` returned **PASS S3/S8** for the final formatted candidate: independently checked raw graph/audit/archives, pinned slice-reader borrowing assumptions, all31 public cases, source/registry/DB ownership and final gate evidence. The bounded XML unit is complete locally; the work-unit commit containing B2 L59 records its source, with observed SHA annotated separately after commit. Protected evidence is under `d12/characterization/baseline-proof` and `d12/migration/{stage-proof,gates-correction1,ownership-correction1,format-correction}` beneath the remediation evidence root. This does not establish native/game, minimum-toolchain, optional-feature runtime, remote CI or a clean audit.


## D13: Wasmtime migration — verified locally

The optional runtime is pinned to `=36.0.19` with upstream default capabilities and `wasm-plugins` preserved. Isolated parent job17 passed the precise solve, complete optional/platform metadata, checksum-verified provisioning and a same-path, fixed-DB, pinned-Cargo/yank-aware audit pair: **27/13 →6/12**. Exactly21 Wasmtime vulnerability records and the genuine removed `paste` warning disappeared. All remaining full finding objects/settings are identical, requiring **zero reference transformations**. The graph has40 added/44 removed identities,720 byte-identical whole records,6 changed-edge and12 qualification-only records; profiling/fxprof0.6/fxhash remains. Candidate lock SHA256: `6645a7809c95d80cdef29168e134b43c8771706c2e3f901e825399f5b035413e`. The parent applied only the core manifest and lock after raw-original backups and exact input/readback checks. This is not runtime acceptance.

The real pre-migration optional baseline passed12/0/1ignored, including four self-contained finite public WAT contracts; the ignored external SDK example is not positive proof. This baseline is the previous source plus292 characterization lines and a two-line XML compile correction: optional compilation exposed ambiguous generic `as_ref()` byte-pointer inference, replaced by the identical underlying byte slice pointer through quick-xml Deref. Earlier snapshot/root/cache refusals, the actual compile RED, and raw logs remain retained. No external SDK was installed and no global or prior-evidence cache was modified.

Job18 stopped at formatting before runtime36 or the suites ran. Raw pre-format copies were retained and rustfmt corrected wrapping/trailing commas. Feature/runtime36, feature-off/XML/full frontend/workspace gates and independent Risk5 verification remain pending; no new source commit, native/game/minimum-toolchain/platform runtime or publication claim. Evidence: protected `d13/{baseline-proof-xml-compile-fix,migration/{stage-proof,apply-proof,gates,format-fix}}` below the remediation evidence root.


D13 parent job20 subsequently passed all serial checks (9m32s): Wasmtime36 contracts12/0/1ignored matching the actual25 baseline, strict feature/all-target Clippy, default-feature guards9/0, XML47/0/TMX16/0/CLI11+4, frontend1126/0/build, strict workspace Clippy, Rust3006/0/25ignored across120summaries, collector10/10 and source/human/cleanup guards. The25 ignored workspace tests and external plugin example were not executed. Preserved job19 first passed runtime36 contracts, then stopped on10 redundant same-type error conversions and1 equivalent extension-predicate lint; narrow corrections keep UTF8 error conversion and custom messages, with no lint suppression. Evidence: `d13/migration/gates-clippy-fixed/`. Independent Risk5 verification and the local source commit remain pending.


Independent verifier `mvpnrosi-d-y06s` returned **PASS S3/S8, no blockers** for the bounded final candidate: recomputed815 unique official archives/index checksums, all500 live source/human hashes, graph/metadata/audit comparisons and staged/apply provenance; pinned tools/clean DB,12,026 seed files and1,400 DB files remain intact. This verifier inspected retained execution rather than rerunning Cargo/npm/scanner. Post-batch cleanup correctly removed the classified15.18GB shared target above its cap; retained proof and six worktrees remain. D13 is complete locally; the source is the work-unit commit containing B2 L69, with observed SHA annotated afterward. **Risk: item5 — dependency update; writer checks and independent verification fulfilled.** D14–D16 remain separate open units.
