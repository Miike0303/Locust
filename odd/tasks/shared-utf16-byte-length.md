# Share the UTF-16 byte-length primitive

## Authorization and boundary
User authorized sequential backlog completion, tests and main push before another unit. Branch `refactor/shared-utf16-byte-length`, base `836446c`. Source: `crates/core/src/validation.rs`, `crates/providers/src/mock.rs`; ledger `docs/IMPROVEMENT-GOAL.md`. No dependencies, new generic module, encoding aliases, saturation, normalization, BOM/terminator addition, game writes, parser/backup/path semantics changes or releases.

## Equivalence and acceptance
The existing Mock helper and core `utf16le` match arm both compute exactly `text.encode_utf16().count() * 2`. Move that primitive to public `locust_core::validation::utf16_byte_len`; core retains `Some` and Mock retains all arguments/comparisons via import. This removes two computational definitions, not two named-function copies. No useful RED exists for behavior-preserving consolidation: characterize GREEN before moving, then retain GREEN; do not stage a failing compile as evidence.

Literal expectations: empty0, ASCII2, CJK2, supplementary emoji4, combining sequence4, NUL2, CRLF4, explicit BOM2, without implicit BOM/terminator. Add a supplementary-character Mock budget case so comparing the helper to itself cannot mask an error. Keep UTF-8/SJIS, UTF-16 truncation, and other superficially similar helpers separate.

## Tasks
- [x] U16-1 — Added characterization tests, observed baseline GREEN, shared primitive and observed focused GREEN. Work unit `6bbca4fdf5ac8050b7cdd1a353069c96999b57ac`; bounded multi-file writer.
- [x] U16-2 — Functional gates passed through command verifier: frontend build/exact suite104/0/0, fmt/strict Clippy, workspace2003/0/19 including Tauri. Native medium/large-writer self-check policy honored; no extra code adjudication. Evidence accompanies documentation work unit.
- [ ] U16-3 — Publish feature, observe both hosted jobs, non-forced main delivery and evidence closeout. **In progress.** No next unit before exact candidate succeeds and remote main confirms.

## Checks and routing
Focused: `cargo test --offline -p locust-core --lib validation::tests`; `cargo test --offline -p locust-providers --lib mock::tests`. Normal generated test outputs are authorized only under gitignored `target/debug`, target metadata, or parent-approved verifier caches; no installs/toolchain changes. Technical source/tests in English. Expected 60-100 authored diff lines, ceiling200 before re-scope. Parent owns tasks/mirror/delivery.

## Evidence and next step
- Explorer proved exact expression, public module and existing dependency; production call arguments/comparisons unchanged. UTF-8/SJIS, truncation and filesystem primitives remain separate.
- Writer baseline and post-refactor: core31/31, Mock9/9, 0 failures/ignored. Core substring filter includes eight existing font-validation tests. Literal Unicode expectations and exact placeholder outputs verified; no fabricated RED.
- Writer used installed Rust1.94 and offline Cargo with repository debug target, no toolchain selection/install or release writes. Targeted rustfmt and diff check passed. Source/ledger diff72 additions/6 deletions (78 lines); Cargo.lock unchanged.
- Staged intended task doc before native assessment: medium, large writer/runtime, executable change,98 lines/four paths, RDDoff/unknown review, no independent code verifier required. Writer self-verification stands; no native review or switch change. Full functional gates still run.
- Command verifier Windows functional gates: npm build, fmt, strict offline workspace Clippy and full offline workspace tests passed (2003/0/19, including Tauri). Frontend original npm unit command exited1/ENOTCACHED before any tests because verifier isolated an empty npm cache; original failure retained.
- Separate read-only incident exploration located existing cached tsx4.23.15. Accepted only normal generated temp caches and host-tool stdout spills, preserved without cleanup. Guarded direct Node invocation used exactly all38 files from the original manifest script, removing only its npx launcher; 104/0/0 passed. This does not claim original offline npx was repaired. No installs/downloads/source changes. Logs: host-temp `locust-utf16-verify`.
- Fresh scope checks: only parent task doc modified, Cargo.lock/index clean. Next: commit evidence, publish feature and observe exact hosted candidate. Broader helper backlog stays pending; no hosted/main claim yet.
