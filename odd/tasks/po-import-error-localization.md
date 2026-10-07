# Localize malformed PO import errors

## Intent and scope
User authorized sequential backlog completion, checks and main delivery before another unit. This handles only owned malformed PO quoted-string errors; generic IO and other parse families remain pending.

Source: `apps/desktop/src/lib/apiError.ts`, `apiError.test.ts`, `api.ts`, `lib/i18n/en.ts`, `es.ts`; ledger `docs/IMPROVEMENT-GOAL.md`. Branch `fix/po-import-error-localization`, base `da5861c`. No backend wire changes, dependencies, blanket regexes, providers, original games, tags/releases or unrelated repositories. Forecast below 200 authored diff lines; observed source diff 170.

## Tasks
- [x] PO-1 — Observe RED, implement narrow Spanish mapping and preserve raw Tauri diagnostics, observe GREEN. Work unit `375ca4a50346debd3eea248fae18be4b271f3ecd`.
- [x] PO-2 — Independently verify tests/build, logging, catalog parity, negatives and actual browser behavior. Verification accompanies work unit `375ca4a`; documentary evidence in this file.
- [x] PO-3 — Publish feature, observe hosted Linux/Windows CI and deliver main. Exact candidate `530d7053f9da6cf63d297fc26f5fa715e342531e` passed both jobs and was confirmed on remote main. Evidence reconciled in the documentation closeout.

## Acceptance and observed checks
- Full owned `parse error in po: line N: malformed PO string: EXCERPT` structure only. Positive one-based line retained as string; full match rejects terminal newline. Recognize current HTTP frame before legacy trimming.
- Spanish HTTP and bare Tauri guidance identifies malformed quoted PO text and line. Excerpt is opaque and excluded from guidance; raw matched Tauri detail logged locally. HTTP raw logging remains single-entry. English and unknown/detection errors unchanged.
- Writer observed meaningful RED twice (3 failed/4 passed), then focused GREEN 7/7 and full frontend 104/104. No tests were ignored or dependencies installed.
- Independent focused 7/7, full frontend 104/104/0 skipped, TypeScript/Vite build and diff check passed. Negatives include other files/reasons, invalid or huge lines, leading text and newlines; diagnostics cover Unicode, quotes, `$&`, paths and trailing spaces.
- Independent headless actual built frontend 3/3: Spanish malformed-PO toast with line 2/no raw excerpt, unknown XLIFF fallback unchanged, English exact old frame. Each Activity log preserved original raw response; zero page exceptions/unexpected requests. Mocked backend, not live parser/native Tauri execution. Actual production API/Tauri seam additionally covered by unit integration.
- Browser harness attempts exited 1,1,1,0,0; setup failures (Windows path, accessible-name shortcut, POST mock status) were fixed only in temporary artifacts. Evidence retained under host-temp `locust-po-error-verify`; all owned browser/server processes closed.
- Initial native assessment unassessable due undeclared untracked task document: treated as high, writer self-checks plus independent verifier completed. RDD stays off. No native review started.
- [Hosted CI run 37551095569](https://github.com/Miike0303/Locust/actions/runs/37551095569), exact SHA `530d7053f9da6cf63d297fc26f5fa715e342531e`, completed successfully: Ubuntu Rust 1989/0/19, Windows Rust 1999/0/19, frontend 104/0/0 on each. Clean npm ci/build/unit, Rust fmt, strict workspace Clippy and workspace tests including Tauri passed. Ignored tests did not run. Evidence: host-temp `locust-po-error-hosted`.
- Fetched main, checked ancestry, fast-forwarded locally and pushed without force; remote main confirmed at exact hosted SHA. Documentation closeout changes no workflow or code from that candidate.
- Nonblocking warnings: stale Browserslist, Vite mixed imports, action runtime and Git LF/CRLF advisories; no compiler errors. npm reports 13 vulnerabilities, untouched in this unit.

## Next step
Publish documentation-only closeout, then take the next bounded backlog unit. This PO family is complete; generic IO and other parse families remain separate.
