# Share the UTF-16 byte-length primitive

## Authorization and boundary
User authorized sequential backlog completion, tests and main push before another unit. Branch `refactor/shared-utf16-byte-length`, base `836446c`. Source: `crates/core/src/validation.rs`, `crates/providers/src/mock.rs`; ledger `docs/IMPROVEMENT-GOAL.md`. No dependencies, new generic module, encoding aliases, saturation, normalization, BOM/terminator addition, game writes, parser/backup/path semantics changes or releases.

## Equivalence and acceptance
The existing Mock helper and core `utf16le` match arm both compute exactly `text.encode_utf16().count() * 2`. Move that primitive to public `locust_core::validation::utf16_byte_len`; core retains `Some` and Mock retains all arguments/comparisons via import. This removes two computational definitions, not two named-function copies. No useful RED exists for behavior-preserving consolidation: characterize GREEN before moving, then retain GREEN; do not stage a failing compile as evidence.

Literal expectations: empty0, ASCII2, CJK2, supplementary emoji4, combining sequence4, NUL2, CRLF4, explicit BOM2, without implicit BOM/terminator. Add a supplementary-character Mock budget case so comparing the helper to itself cannot mask an error. Keep UTF-8/SJIS, UTF-16 truncation, and other superficially similar helpers separate.

## Tasks
- [ ] U16-1 — Add characterization tests, observe baseline GREEN, share primitive, observe focused GREEN. **In progress.** Bounded multi-file writer.
- [ ] U16-2 — Independently verify focused cases, full fmt/Clippy/workspace tests and source scope. Frontend build before workspace/Tauri commands; existing installed toolchain only. Follow native assessment; no implicit RDD enablement.
- [ ] U16-3 — Commit, publish feature, observe both hosted jobs, non-forced main delivery and evidence closeout. No next unit before exact candidate succeeds and remote main confirms.

## Checks and routing
Focused: `cargo test --offline -p locust-core --lib validation::tests`; `cargo test --offline -p locust-providers --lib mock::tests`. Normal generated test outputs are authorized only under gitignored `target/debug`, target metadata, or parent-approved verifier caches; no installs/toolchain changes. Technical source/tests in English. Expected 60-100 authored diff lines, ceiling200 before re-scope. Parent owns tasks/mirror/delivery.

## Evidence and next step
Read-only explorer proved identical expression, existing public validation module and providers-to-core dependency; callers need no adaptation. Existing fixtures omit supplementary characters and compare some budgets through the same helper. Next: characterization-first writer. Broader helper consolidation remains pending; no tests or implementation claimed yet.
