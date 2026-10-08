# Prepare Rule95 patches from Locust desktop

## Intent and boundary
User selected desktop publishing preparation: expose DLsite identity/version and generate a catalog-ready ZIP plus optional Rule95 Markdown entry. Actual upload stays in the separate site publisher. Authorized delivery: work branch -> manual hosted CI -> fast-forward main. Branch `feat/desktop-publish`, base `985b73e`.

Preserve originals, CLI/manifest compatibility and existing changes. English technical artifacts, neutral Spanish UI. No credentials, live upload, purchases, original-game modification, releases or native UI claims.

## Recovery and workload
This document was created during final verification after `ea8b156`, not before implementation. Pre-write tracking was missed; the parent also performed DB-path corrections and broad verification inline despite mandatory delegation. Remaining correction/checks and ledger writing use bounded agents. Do not fabricate earlier tracking or delegated evidence.

Initial unit: 16 files, 1192 additions/165 deletions, including shared core/CLI rendering and tests; exceeded the workload heuristic without recorded size decision. Preserve tested history; no cosmetic shrinking or rewriting. No PR is created; user-authorized main delivery is the boundary. Quote correction: six files, 88 additions/8 deletions.

## Tasks
- [x] PUB-1 — Desktop identity/entry preparation, shared core/CLI logic, DB-backed packing regression. Work unit `ea8b15688c7ae4eb71caf79cd3bcedbfc2a2a9f9`; Codex writer plus parent corrections.
- [x] PUB-2 — PowerShell literal command quoting including typographic apostrophes and explicit UI shell label. Work unit `a8da02747cf9e52ad1e042ee1018a7abc60daeb3`; delegated writer and independent verifier, outcomes below.
- [x] PUB-3 — Exact source `a8da02747cf9e52ad1e042ee1018a7abc60daeb3` passed hosted run37708121668 and was fast-forwarded/pushed to main; remote SHA confirmed. Delegated ledger writer and independent structural verifier passed. This document accompanies the passive closeout work-unit commit; code is unchanged from the tested source. No meaningful RED for passive docs.

## Acceptance
- Desktop offers RJ detection/edit/opt-out, version, optional entry path/save picker and copyable PowerShell command.
- Only the live project DB path normalizes to selected/sole recording root. Other paths retain mismatch refusal.
- Entry protects game/pristine/backup trees, DB/sidecars and ZIP; no overwrite of existing entry.
- Clean game title, actual engine/identity and ZIP SHA-256 in entry. Translator completes blocking metadata first.
- Shell quoting preserves original path including ASCII/typographic apostrophes, dollars and backticks.

## Verified evidence
- DB-path regression: parent observed RED400 then GREEN; HTTP publishing7/7, protected-entry/unrelated-root refusal. Independent verifier repeated HTTP7/7 and diff checks.
- Local Rust2032/0/19, fmt and strict workspace Clippy green. Initial frontend108/0 and production build green.
- [Initial hosted run37704501386](https://github.com/Miike0303/Locust/actions/runs/37704501386), exact ea8b156: Ubuntu Rust2022/0/19, Windows2032/0/19; frontend108/0/0 each; both jobs successful. Ignored tests not executed.
- Real backend/headless production UI on scratch fixture: open saved DB -> strict Ren'Py ZIP+entry; RJ01234567, version1.0.2, title Demo Game, two fingerprints; zero page errors. No native Tauri/game runtime claim.
- Actual Rule95 dry-run rejected TODO creator, then succeeded after scratch-only metadata edit. No R2 upload.
- Quote correction: initial ASCII RED focused2/8fail then GREEN10/0. Independent PowerShell AST exposed U+2018/U+2019 failure2/2. Final writer RED focused11/5fail, AST12/37fail; GREEN focused16/0, frontend123/0/build good.
- Fresh independent focused16/0 and PowerShell5.1.26100.9444 AST+no-op capture49/0 using regenerated current-helper commands. Five recognized delimiters (ASCII plus U+2018-U+201B), all25 adjacent combinations, dollars/backticks/semicolon preserved. No actual publisher/external npm execution for this check. Evidence: host-temp locust-pub2-verifier-ps51-01.
- Build warnings: stale Browserslist and mixed static/dynamic imports; diff checks green with LF/CRLF advisories.
- Final [hosted run37708121668](https://github.com/Miike0303/Locust/actions/runs/37708121668) at exact a8da027: Ubuntu Rust2022/0/19, Windows2032/0/19; frontend123/0/0 each. Both passed npm ci/build, fmt, strict Clippy, workspace tests including Tauri. Ignored tests not run. Independent job-log counts verified. npm13 vulnerabilities (1low/6moderate/6high), action runtime deprecations and existing build warnings remain outside scope.
- Native assessment first failed invalid base argument, corrected main-range assessment medium/large writer/RDDoff. Later dirty assessment unavailable due untracked task document; conservatively high, independent verifiers ran for correction and passive ledger. No native review or approval claimed.
- Failed earlier browser setup format selection corrected. DB-path pack corrected. Reopening Direct-injected folder re-extracts modified sources/resets status: separate unresolved follow-up.

## Next step
Feature complete: publish passive ledger/task closeout on the feature branch and fast-forward main with source equality to a8da027; no new source checks claimed for documentation-only commit. Next product steps require separate user direction: Cloudflare login/deployment and first real upload, native Tauri QA, or reproduce injected-folder reopen behavior. No new autonomous unit is authorized by this closure.
