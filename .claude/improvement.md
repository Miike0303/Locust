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

Last verified run: 2026-09-25, `/fleet-explore`, on the uncommitted working tree. All green.
Wall-clock times on this machine:

| Gate | Result | Time |
|---|---|---|
| `npm run build` | pass | 18s |
| `npm run test:unit` | pass, 31 files | 337s |
| `cargo fmt --all --check` | pass | 6s |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass | 76s |
| `cargo test --workspace` | 1453 passed, 0 failed, 17 ignored, 59 suites | 104s (warm) |

CI runs the same five commands in the order build → unit → fmt → clippy → test
(`.github/workflows/ci.yml:49-66`). The frontend build must run first because
`tauri-build` reads `apps/desktop/dist`.

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
- **Stage exactly the cycle's paths.** Never `git add -A` or `git commit -a`.
  `tmp/` (huge QA evidence) is excluded only through `.git/info/exclude`, and
  stray `*.locust.db` files sit in `apps/desktop/`. The backlog of uncommitted
  work since 2026-09-03 was snapshotted in `e0e5268..` on 2026-09-26. A dirty
  tree at cycle start means someone else is working here: stop.
- **Writers:** as of 2026-09-26 the user authorized Codex (`codex-write.ps1`) and
  Cursor (`cursor-write.ps1`). Grok Build stays off. Frontend is Claude's per
  the fleet rules.
- `bat`/`fd`/`eza` are not installed on this Windows host. Use `rg` and the Read tool.
- Isolate every QA run with `LOCUST_DATA_DIR=<scratch>`. Without it the run uses
  the user's real profile, keys and global memory (`crates/core/src/config.rs:183`).
- Browser-mode UI needs `locust server --port 7842`, because production
  `api.ts:41` hard-codes 7842 while the CLI defaults to 3000 (`crates/cli/src/main.rs:275`).
  Vite dev runs on 1420 and proxies `/api` to 7842. Folder pickers fall back to
  `window.prompt`, so headless QA must answer `dialog` events.
- Playwright is available as `py -3.13` with its bundled Chromium. The default
  `python` has no Playwright.
- A new placeholder syntax needs a detector in
  `crates/core/src/placeholder.rs`. Validation only flags what the detectors
  extract, so a dropped unknown token passes silently.
- Test counts in `CLAUDE.md` and the docs disagree (890/1239/1439/1448/1453).
  Trust only a run you did yourself.

## Stopping

The loop runs until the user stops it, the session closes, or there is nothing
actionable left — an empty backlog **and** a research round that found nothing.
Nothing else ends it. In particular a completed cycle does not: re-arm at the
end of every turn.

Cycles 3 and 4 ran only because the user prompted, after the tick was never
re-armed. That failure is silent by nature — a loop that stopped looks the same
as a loop with nothing to say.

## Notes

`delivery: commit-only` and `branch: feature` are deliberate for this repo:
commits land on the current feature branch, never on `main`, and nothing is
pushed. This is stricter than ThreeMaker, which pushes because CI gates every
push there. Do not harmonise them.
