# Share the UTF-16 byte-length primitive

## Authorization and boundary
User authorized sequential backlog completion, tests and main push before another unit. Branch `refactor/shared-utf16-byte-length`, base `836446c`. Source: `crates/core/src/validation.rs`, `crates/providers/src/mock.rs`; ledger `docs/IMPROVEMENT-GOAL.md`. No dependencies, new generic module, encoding aliases, saturation, normalization, BOM/terminator addition, game writes, parser/backup/path semantics changes or releases.

## Equivalence and acceptance
The existing Mock helper and core `utf16le` match arm both compute exactly `text.encode_utf16().count() * 2`. Move that primitive to public `locust_core::validation::utf16_byte_len`; core retains `Some` and Mock retains all arguments/comparisons via import. This removes two computational definitions, not two named-function copies. No useful RED exists for behavior-preserving consolidation: characterize GREEN before moving, then retain GREEN; do not stage a failing compile as evidence.

Literal expectations: empty0, ASCII2, CJK2, supplementary emoji4, combining sequence4, NUL2, CRLF4, explicit BOM2, without implicit BOM/terminator. Add a supplementary-character Mock budget case so comparing the helper to itself cannot mask an error. Keep UTF-8/SJIS, UTF-16 truncation, and other superficially similar helpers separate.

## Tasks
- [x] U16-1 — Added characterization tests, observed baseline GREEN, shared primitive and observed focused GREEN. Work unit `6bbca4fdf5ac8050b7cdd1a353069c96999b57ac`; bounded multi-file writer.
- [x] U16-2 — Functional gates passed through command verifier: frontend build/exact suite104/0/0, fmt/strict Clippy, workspace2003/0/19 including Tauri. Native medium/large-writer self-check policy honored; no extra code adjudication. Evidence work unit `65df3148bb86bd4d63d3f7819dd211c3136abfe9`.
- [x] U16-3 — Published feature, observed both hosted jobs and delivered main non-forced. Exact tested SHA `65df3148bb86bd4d63d3f7819dd211c3136abfe9` confirmed on remote main; evidence reconciled in documentation closeout.

## Checks and routing
Focused: `cargo test --offline -p locust-core --lib validation::tests`; `cargo test --offline -p locust-providers --lib mock::tests`. Normal generated test outputs are authorized only under gitignored `target/debug`, target metadata, or parent-approved verifier caches; no installs/toolchain changes. Technical source/tests in English. Expected 60-100 authored diff lines, ceiling200 before re-scope. Parent owns tasks/mirror/delivery.

## Evidence and next step
- Explorer proved exact expression, public module and existing dependency; production call arguments/comparisons unchanged. UTF-8/SJIS, truncation and filesystem primitives remain separate.
- Writer baseline and post-refactor: core31/31, Mock9/9, 0 failures/ignored. Core substring filter includes eight existing font-validation tests. Literal Unicode expectations and exact placeholder outputs verified; no fabricated RED.
- Writer used installed Rust1.94 and offline Cargo with repository debug target, no toolchain selection/install or release writes. Targeted rustfmt and diff check passed. Source/ledger diff72 additions/6 deletions (78 lines); Cargo.lock unchanged.
- Staged intended task doc before native assessment: medium, large writer/runtime, executable change,98 lines/four paths, RDDoff/unknown review, no independent code verifier required. Writer self-verification stands; no native review or switch change. Full functional gates still run.
- Command verifier Windows functional gates: npm build, fmt, strict offline workspace Clippy and full offline workspace tests passed (2003/0/19, including Tauri). Frontend original npm unit command exited1/ENOTCACHED before any tests because verifier isolated an empty npm cache; original failure retained.
- Separate read-only incident exploration located existing cached tsx4.23.15. Accepted only normal generated temp caches and host-tool stdout spills, preserved without cleanup. Guarded direct Node invocation used exactly all38 files from the original manifest script, removing only its npx launcher; 104/0/0 passed. This does not claim original offline npx was repaired. No installs/downloads/source changes. Logs: host-temp `locust-utf16-verify`.
- [Hosted run 37558334740](https://github.com/Miike0303/Locust/actions/runs/37558334740), exact candidate `65df3148bb86bd4d63d3f7819dd211c3136abfe9`, succeeded: Linux Rust1993/0/19, Windows2003/0/19, frontend104/0/0 each. Both passed actual npm ci/build/unit, fmt, strict workspace Clippy and workspace Rust tests. Primary harnesses report19 ignored per OS; npm13 vulnerabilities remain untouched.
- Hosted verifier capture root used relative TEMP and landed in repository `tmp/locust-utf16-hosted`, outside the requested host-temp location. Separate incident exploration and fresh parent lstat/resolve checks confirmed six files (1,517,681 bytes), no reparses/symlinks/escape and no source mutation. `.git/info/exclude:19` ignores this tmp root. Artifacts accepted and preserved; no cleanup. Parent additionally machine-checked exact successful binding and all75 Rust summaries per OS. Log-parser encoding/matcher failures were capture-harness issues, not product failures.
- Fetched main, checked ancestry, fast-forwarded locally and pushed without force; remote main confirmed at exact hosted candidate. Documentation closeout changes no code/workflow from that candidate.
- Publish documentation-only closeout, then next bounded unit. This primitive is complete; broader helper families remain pending. Follow-up discovery: unit runner uses undeclared/unpinned npx tsx; reproducibility should be handled separately, not folded into this refactor.
