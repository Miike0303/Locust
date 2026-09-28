---
ledger: docs/IMPROVEMENT-GOAL.md
categories: [capability, optimization, defect]
delivery: commit-only
branch: main
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
| `npm run test:unit` | pass, 31 files | 17s (was 337s before cycle 31) |
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
- **Writers: Codex ONLY (user, 2026-09-27).** No Cursor, no Grok Build. Use
  Codex until its quota and resets are exhausted or the user says otherwise.
  Supersedes the 2026-09-26 Codex+Cursor authorization.
- **Model allow-list (user, 2026-09-27, supersedes 2026-09-26). Never substitute:**
  - Codex writers: `gpt-6-astra` with `-c model_reasoning_effort=xhigh -c service_tier="fast"`.
  - Codex researchers: `gpt-6-sol` with `-c model_reasoning_effort=high -c service_tier="fast"`.
  - Why: the user wants the Codex subscription and its resets used well, so Fast tier and high effort are
    deliberate. The CLI accepts `service_tier="fast"` (`models_cache.json` lists `additional_speed_tiers: ['fast']`,
    API id `priority`); the session log does not record the tier, so it is unconfirmed beyond "accepted".
  - Loop cadence: 15-20 minute ticks (user, 2026-09-27).
- `bat`/`fd`/`eza` are not installed on this Windows host. Use `rg` and the Read tool.
- Isolate every QA run with `LOCUST_DATA_DIR=<scratch>`. Without it the run uses
  the user's real profile, keys and global memory (`crates/core/src/config.rs:183`).
- Browser-mode UI needs `locust server --port 7842`, because production
  `api.ts:41` hard-codes 7842; since cycle 42 the CLI default is 7842 too (it was 3000).
  Vite dev runs on 1420 and proxies `/api` to 7842. Folder pickers fall back to
  `window.prompt`, so headless QA must answer `dialog` events.
- **Codex CAN run the full suite with a widened sandbox (since cycle 37).** Call
  `codex exec` directly, because the `codex-write.ps1` wrapper cannot pass these
  flags:
  `-s workspace-write -c "sandbox_workspace_write.writable_roots=['C:/Users/Mike/AppData/Local/locust-patch-locks-v1','C:/Users/Mike/AppData/Local/Temp']" -c "sandbox_workspace_write.network_access=true"`.
  Use TOML single-quoted strings; PowerShell 5.1 strips embedded double quotes.
  Never add `%LOCALAPPDATA%\project-locust` to the writable roots: its being
  unwritable is what proves the tests do not touch the real profile. With plain
  `workspace-write` about 111 tests fail (the lock dir, and no loopback for the
  server tests).
- **Full-Codex cycle mode (user, 2026-09-27: "use Codex much more").** One Codex
  gpt-6-sol session researches, picks, implements and runs all five gates in the
  widened sandbox. Claude reads the report, reviews any production hunk outside
  the brief, reruns the gates once and commits. Report this as "research duel
  skipped (single researcher)".
- **Use `cursor-ask -Mode ask`, not `plan`, for research.** `plan` returned empty
  stdout (exit 0) on the long brief twice, with both cursor-grok-4.6-xhigh and
  grok-4.7-high. `ask` with the same brief reads the repo and answers. Always
  check the output size, and never count an empty answer as a proposal.
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

`delivery: commit-only` and `branch: main` (user, 2026-09-27: "cambia a rama
main, y sigue"; main was fast-forwarded from `feat/desktop-ux-kimi-k3-p2`).
Commits land on local `main`; nothing is pushed. This is stricter than ThreeMaker, which pushes because CI gates every
push there. Do not harmonise them.
