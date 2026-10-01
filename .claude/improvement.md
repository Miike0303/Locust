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
- **Writers: Codex first (user, 2026-09-27); Cursor Grok 4.7 only as the fallback below (user, 2026-09-28).** No Grok Build. Use
  Codex until its quota and resets are exhausted or the user says otherwise.
  Supersedes the 2026-09-26 Codex+Cursor authorization.
- **Fallback: Cursor Grok 4.7 when Codex is unavailable (user, 2026-09-28: "usa cursor, grok 4.7 si codex no esta
  disponible").** Codex stays primary. "Unavailable" means: the trivial `codex exec` probe gets no answer within
  180 s, a Codex session stalls or fails the sandbox twice in a row for the same step, or Codex quota is exhausted.
  Then use Cursor: writers `~/.claude/bin/cursor-write.ps1 -WorkDir <repo> -Model grok-4.7-xhigh -PromptFile <brief>`;
  researchers `~/.claude/bin/cursor-ask.ps1 -WorkDir <repo> -Mode ask -Model grok-4.7-high -PromptFile <brief>`
  (`ask`, not `plan`: plan returned empty output before). Never the `-fast` variants. Probe Codex again at the next
  step and switch back as soon as it answers. Cursor Ultra is credit-metered: only as a fallback, never in parallel with
  a Codex writer on this tree. Grok historically stops at the TDD red phase (see above): check the suite is not red
  before accepting its report, and expect one `-Continue` dispatch.
- **Model allow-list (user, 2026-09-27, supersedes 2026-09-26). Never substitute. No Fast tier:**
  - **Updated 2026-09-29 (user: "ya esta el modelo 6.1 sol, cambialo"; needs Codex CLI >= 0.159.1): Sol 6.1 replaces Astra.**
  - Codex writers: `gpt-6.1-sol` with `-c model_reasoning_effort=xhigh`.
  - Codex researchers and any subagent: `gpt-6.1-sol` with `-c model_reasoning_effort=high`.
    Do NOT use `gpt-6-sol` (6.0) or `gpt-6-astra` any more (superseded; the earlier "never gpt-6-sol" rule meant the 6.0 model).
    Probe: `codex exec -m gpt-6.1-sol -c model_reasoning_effort=high "Reply with the single word: ok" </dev/null` answered ok on 2026-09-29.
  - The user tried and then declined `service_tier="fast"`; leave the default tier.
  - Loop cadence: 15-20 minute ticks (user, 2026-09-27). **Updated 2026-09-29: 20-minute ticks; Claude double-verifies every writer result and, if it is wrong, fixes it directly instead of a correction round (user).**
- **Codex Windows sandbox fails under heavy concurrency (2026-09-27).** With ~20 Codex sessions from other
  projects (ThreeMaker, Runnked-backend, Fantasy Cut, MikeLibrary fleets) running at once, a cycle writer failed
  with `windows sandbox failed: helper_unknown_error: apply deny-read ACLs`, and shells hung; it exits 0 with a
  report saying nothing was done. Run at most ONE Codex session from this loop at a time when other fleets are busy,
  grep every report/log for `sandbox failed`, and never kill Codex processes you did not start (check
  `Win32_Process` parent and `-C <repo>` first). A trivial `codex exec ... "echo hello"` probe tells you if it recovered.
- **Linux check (cycle 63).** CI is ubuntu, and `cargo test` stops at the first failing binary, so CI hides later
  failures. Reproduce locally with Docker (WSL Ubuntu has no C toolchain and `sudo` needs a password; do not use
  `wsl -u root`): clone to scratch, then `MSYS_NO_PATHCONV=1 docker run --rm -v <clone>:/src -v locust-linux-target:/target
  -v locust-cargo-reg:/usr/local/cargo/registry -e CARGO_TARGET_DIR=/target -w /src rust:1.94.0 bash -c 'cargo test
  --workspace --exclude locust-desktop --no-fail-fast'`. Without `MSYS_NO_PATHCONV` Git Bash rewrites `/src`. The desktop
  crate needs `apt-get install libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libssl-dev` in the container
  and a copied `apps/desktop/dist`. Tests that spawn a child (`Command::new(current_exe())`) share a process with
  lock tests: flock fds leak across fork until exec, so run suspect binaries ~20x in parallel before trusting green.
- Codex stops on "ACL errors" if the brief says so: git always warns it cannot read `~/.config/git/ignore` in the
  sandbox. Stop rules must name `windows sandbox failed` exactly.
- `bat`/`fd`/`eza` are not installed on this Windows host. Use `rg` and the Read tool.
- **Ground-truth audits (2026-09-29, the most productive technique so far).** Before writing a brief from code
  reading, look for a real artifact on disk that lists what the engine itself treats as translatable (Ren'Py: the
  `tl/<lang>/*.rpy` files it wrote; RPG Maker: original vs translated pairs plus an independent JSON parse), measure
  recall and precision with a script, give the writer the script and demand BEFORE/AFTER numbers, then re-run it
  yourself. Scripts, how to run and the latest reports: `tools/ground-truth/README.md`. Ren'Py went from recall
  65.96% / precision 88.15% to 97.92% / 98.45%. Researchers doing this may write only under the scratch directory,
  never the repo or `D:\juegos`; check `git status --short` afterwards.
- **Briefs go through the file-writing tool, not shell heredocs.** A backtick or apostrophe in a briefing broke bash
  twice (`unexpected EOF`, nothing started). Launch with `nohup codex exec ... - < brief.md > log 2>&1 &`.
- **`docs/IMPROVEMENT-GOAL.md` has mixed line endings** (mostly LF, ~35 CRLF, stray CR characters inside old entries).
  Edit it by whole `\n`-delimited physical lines (Python `split('\n')`), never `splitlines()`, `split('\r\n')` or
  multi-line perl, and check `git diff --stat` (a wrong edit once showed +37/-67 for three lines).
- **Codex sandbox/update on this host:** `[windows] sandbox = "elevated"` made some reads hang ~170 s while
  `unelevated` answered in 20 s; try `-c windows.sandbox="unelevated"` per call only after rerunning the full suite.
  `codex update` fails from pwsh 7 (`Get-FileHash` missing) unless
  `$env:PSModulePath = "$env:WINDIR\System32\WindowsPowerShell\v1.0\Modules"` is set first.
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
  gpt-6-astra session researches, picks, implements and runs all five gates in the
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

- **Concurrency cap (2026-09-30).** At most 2 Codex sessions at once (writers + researchers together) and none of your own cargo builds beside them: with 3 sessions plus verification builds the Codex command runner hung in all three (log stops growing, 'command still pending'). Verify a finished writer when at most one session is running. Writers run in scratch snapshots, never cargo in the repo directory. Parallel writers must own DISJOINT files; restrict a researcher brief's 'Files to edit' to one file per lane before dispatching.
- **Stop rules in briefs (2026-09-30).** Never write `if you see the text "X", stop`: a researcher that greps the repo finds it in other briefs and aborts. Write 'stop only on a real tool error in your own calls' and forbid searching the repo's `tmp/`.
- **Process stops need the user (2026-09-30).** The auto-mode classifier blocks `Stop-Process` on Codex processes it cannot prove are the agent's own (other fleets share the machine). List them read-only and ask.
- **Verification recipe that worked (2026-09-30).** `git worktree add -f --detach <Temp>/claude-verify-X HEAD`; copy only that writer's file plus `apps/desktop/dist`; run fmt/clippy/test in a background script with `CARGO_TARGET_DIR=<Temp>/claude-verify-target`; wait with `until rg -q DONE out; do sleep 5; done`; `git worktree remove --force`.
- **Ledger edits via a Python FILE script** (split on a newline only, placeholder `PENDINGnnn` hash replaced with `sed` after the commit, check for U+FFFD). Backslash-U inside an inline Python string literal with a Windows path is a syntax error: use forward slashes or raw strings, and write the script with the file tool.
- **Audit tooling for Unity and the VN engines** lives in `tools/ground-truth/unity` and `tools/ground-truth/vn` (the UnityPy venv is recreated by `bootstrap.py`).

- **Model update (user, 2026-10-01: "si cambie a astra con priority eh fast, checalo"). Supersedes the allow-list above and the "no Fast tier" lines.** Codex writers: `gpt-6-astra` with `model_reasoning_effort=xhigh`; researchers and subagents: `gpt-6-astra` with `high`; service tier `priority` (Fast), which is the default of `~/.claude/bin/codex-write.ps1` (`-Model gpt-6-astra -ServiceTier priority`) and of `~/.codex/config.toml`. Do not pass `-Model gpt-6.1-sol` any more. Fast uses the plan quota faster: read `fleet-quota.ps1` before dispatching and ask the user before continuing if Codex `used_percent >= 90`. Cursor fallback is unchanged (`grok-4.7-xhigh`/`-high`, never `-fast`). Verified 2026-10-01: wrapper defaults and config.toml both say astra + priority; the 2026-10-01 cycle 113/114 runs passed `gpt-6.1-sol` but no `-ServiceTier`, so they ran with `priority` anyway.
- **Scratch hygiene (2026-10-01, after 44 folders = 309.8 GB were left in `%TEMP%`).** Each old scratch root held its own full cargo `target/` with debuginfo (25-41 GB). Rules for every researcher, writer and verification: (1) one scratch root per role named `%TEMP%\locust-<cycle>-<role>` (for example `locust-c113-unity-reaudit`), kept under 5 GB; (2) never create a cargo target inside it: build with `CARGO_TARGET_DIR=%LOCALAPPDATA%\locust-shared-target CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0` (`tools/ground-truth/unity/run_all.py` already does) and copy only the finished exe; (3) never copy a whole game, copy only the files a probe needs; (4) briefs must include this rule and the final `tools/cleanup-scratch.ps1` report-only run; (5) Claude runs `powershell -NoProfile -File tools/cleanup-scratch.ps1 -Apply` at the END of every cycle (it removes `locust-*` folders older than 2 h without a `.keep` file, deletes the shared target above 15 GB, and prunes worktrees) and again at the START of the first cycle of a session; (6) `git worktree remove --force` every verification worktree in the same step it was created for. The `Temp\claude` session folder is Claude Code's own and is not covered here.

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
