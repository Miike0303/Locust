---
ledger: docs/IMPROVEMENT-GOAL.md
categories: [capability, optimization, defect]
delivery: commit-only
branch: feature
push_refs: []
---

## Gates

All five, in order. Run them yourself; never trust a writer's report.

```sh
export PATH="$PATH:/c/msys64/mingw64/bin:/c/Users/Mike/.cargo/bin"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cd apps/desktop && npm run build
cd apps/desktop && npm run test:unit
```

`cargo check` without `--all-targets` does not compile test code. It proves
nothing, and it is not a substitute for the first gate.

## Shared files

Owned by no lane; Claude applies these after the writers finish.

- `Cargo.toml`, `Cargo.lock`
- `apps/desktop/package.json`, `apps/desktop/package-lock.json`
- `apps/desktop/src/lib/i18n/en.ts`, `apps/desktop/src/lib/i18n/es.ts`
- `.github/workflows/*`

## Writer traps

- **Grok stops at the TDD red phase.** It writes the failing tests, then ends the
  turn without implementing. Observed on single-task briefs too, so "one task per
  dispatch" does not prevent it. Expect a second `-Continue` dispatch per change,
  and check for a red suite before assuming the work is finished.
- A writer that exits `0` may still have produced nothing — a lost connection ends
  the process cleanly. Check the output size, not the exit code.
- `httpmock`'s matcher is `body_contains`, not `body_includes`.
- New user-facing strings need keys in BOTH `en.ts` and `es.ts`.
- Never propose or accept an unattended write to the user's own game files —
  batch writes need a human.

## Notes

`delivery: commit-only` and `branch: feature` are deliberate for this repo:
commits land on the current feature branch, never on `main`, and nothing is
pushed. This is stricter than ThreeMaker, which pushes because CI gates every
push there. Do not harmonise them.
