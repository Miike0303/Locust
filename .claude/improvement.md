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
- `apps/desktop/src/i18n/en.ts`, `apps/desktop/src/i18n/es.ts`
- `.github/workflows/*`

## Writer traps

- `httpmock`'s matcher is `body_contains`, not `body_includes`.
- New user-facing strings need keys in BOTH `en.ts` and `es.ts`.
- Never propose or accept an unattended write to the user's own game files —
  batch writes need a human.

## Notes

`delivery: commit-only` and `branch: feature` are deliberate for this repo:
commits land on the current feature branch, never on `main`, and nothing is
pushed. This is stricter than ThreeMaker, which pushes because CI gates every
push there. Do not harmonise them.
