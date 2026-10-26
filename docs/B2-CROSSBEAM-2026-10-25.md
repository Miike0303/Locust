# B2 scanner recovery and withdrawn Crossbeam candidate

The project-owned audit prerequisites were recovered. The dependency candidate was **withdrawn**, not delivered: the bounded comparison did not establish equivalent registry/yank coverage. [B2 D8/D9](../odd/tasks/b2-dependencies.md) owns the outcome; [AUTO-4/AUTO-11](../odd/tasks/autonomous-2026-10-25.md) coordinates this continuation. No publication or quota exhaustion is claimed.

## Verified recovery (D8 complete)

- Job30 exited0: installed `cargo-audit 0.22.2`, binary SHA256 `6b25473466eda7e1dcde0c361545420a6cf866fb5cf0d0d9bcd00e289696cc7f`.
- Official advisory DB: clean pinned commit `21d909e4c8bd4e14c7aea58bf980c881a0053ab1`; anonymous clone, no forwarded credential/Git configuration.
- Initial real workspace scan: collector0, scanner1, **30 vulnerability records / 17 warnings**. Collection success is not a clean audit. Exactly one target record is `RUSTSEC-2026-0204`, `crossbeam-epoch 0.9.18`; patched range `>=0.9.20`, unaffected `<0.9.0`.
- Scanner/profile/Cargo home are disposable project-owned paths. Installed Rust1.94/MSVC were used without SDK installation or mutation; only the existing approved shared Cargo build target was used. Initial GNU `link.exe` selection failed twice (jobs28/29); standard installed `vcvars64.bat`, explicit MSVC linker and a successful compile/run probe resolved it. All pre/post classified cleanups returned0.
- Scanner archive checksum was verified against the approved official value. No global SDK registry/cache, secrets, real provider/game, signing, auth design or CI-trigger changes.

## Structurally valid candidate, incomplete acceptance (D9 blocked)

Official epoch0.9.20 archive SHA256: `2d6914041f254d6e9176c01941b21115dcfb7089e55135a35411081bd106ef3f`; not yanked in the retained official metadata, declared MSRV1.61. Its normalized source manifest requires utils `^0.8.18`, compatible with locked0.8.21. The only locked epoch parent, deque0.8.6, requires `^0.9.17`.

Cargo's isolated precise update and locked offline metadata exited0. Only epoch version/checksum changed; **781 unrelated whole records and all other lockfile bytes were identical**. Candidate SHA256: `36f263af2ea4172e1ea3c45aba594b1d411ec5c3c9a7bab85971b80c1e2c455d`.

| Observed input | Vulnerability records | Warnings | Result |
|---|---:|---:|---|
| Initial workspace baseline, old lock | 30 | 17 | Findings, scanner1 |
| Workspace candidate after Cargo index refresh | 29 | 18 | Findings, scanner1; comparison guard failed |
| Isolated old/new pair, protocol unspecified | 30 → 29 | 17 → 17 | Failed required18-warning guard |
| Isolated old/new pair, sparse protocol aligned | 30 → 29 | 17 → 17 | Failed same guard; budget exhausted |

The workspace candidate preserved all prior17 warning objects and exposed a new **yanked `chacha20 0.10.1`** warning for an unchanged package. Neither isolated pair certified preservation of that observed warning. The exact workspace/registry-context difference remains unresolved; the sparse-only hypothesis was insufficient. Do not suppress the warning, infer a solver incompatibility, or call these partial checks PASS.

Retained harness failures are distinct from a product defect: job31 assumed utils exactly0.8.21 instead of reading the actual compatible source requirement; job32 applied the valid minimal candidate but rejected its warning mismatch; jobs33/34 retained the failed isolated comparisons. A cosmetic source-string assertion during SDK-helper preparation failed before any write; subsequent actual AST/literal checks passed.

## Safe final state and decision

Only our two `Cargo.lock` edits were withdrawn after checking exact candidate ownership. Main was restored byte-for-byte to baseline SHA256 `8a6e83359cfee76a09ee40c315946f136fb9805438ce9f06c649b77cdb4ffdaf`, still containing epoch0.9.18. The candidate, raw stdout/stderr, child statuses, registry-index fingerprints and withdrawal record remain under `.test-tmp/continuation-20261025-evidence/`; no source tests, evidence or worktrees were deleted.

**Not run:** candidate frontend/Rust full gates and the independent Risk5 verifier. Assessment required both self-verification and an independent verifier; failed prerequisite proof and withdrawal prevented delivery. The verifier was not claimed unavailable. No dependency commit was made. Existing UI/runtime proofs belong to sourcec5cf3c4/reportc11d190 and are not fresh dependency acceptance.

**Needs your decision:** renew one bounded investigation of audit workspace/registry resolution before reconsidering D9, or leave the candidate withdrawn. There are no further automatic scans or correction attempts in this exhausted unit. This is not minimum-Rust, optional-plugin runtime, all-platform, native-game or exploitability certification.

Evidence roots: `b2-recovery{,-correction1,-correction2}/`, `b2-collector-selfcheck/`, `b2-crossbeam/`, `b2-crossbeam-correction1/{stage-result.json,comparison/,comparison-sparse/,withdrawal.json}` beneath the continuation evidence directory. Public collector selfcheck:10/10, exit0; not a vulnerability scan.
