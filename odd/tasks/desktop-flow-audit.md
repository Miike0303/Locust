# Desktop flow audit and sequential improvements
Objective: audit current screens with isolated runtime evidence, then fix reproduced defects in priority order.
Branch: audit/desktop-flow-20261016; delivery: auto-chain / stacked-to-main, exact-candidate CI before main; checks: frontend unit/build, Rust workspace/fmt/Clippy, isolated browser screenshots.

## Specs
- S1: "analiza el codigo, las sesiones, levanta la app, haz capturas de pantalla de cada paso"
- S2: "dime que pantallas ya tengo, cuales fallan, cuales se pueden mejorar, cuales se faltan por desarroollar, en mecanicas, sistemas, etc"
- S3: "haz una lista de tareas que seguiras y haras en todo el dia, para dejarlo solo, e ir subiendo todo a main, cada tarea cuando pase al 100% y los test no den errores, ni de codigo ni visual, pasar a la siguiente tarea automaticamente en cadnea hasta terminar todas las tareas"
- S4: "si salen bugs o fallos entre medio priorizarlas y resolverlas antes de pasar a la siguietne tarea"
- S5: "que la navegacion sea fluida y sin compliaciones, que siemrre se tenga un orde"
- S6: "ir borrando los test y cache viejos en cad anuevo test para no llenar de basuraa el disco en cada test y despues no borraarlo"

## Tasks
- T1 [S1,S2,S6] pending — Audit screens, reproduce navigation states, capture isolated evidence and publish inventory. Route: inline runtime audit + delegated read-only mapping (evidence budget); commit: pending.
- T2 [S3,S4,S5] in_progress — Fix unknown-route blank screen with regression and visual proof; preserve all existing routes. Route: inline unless high-risk verification fires; commit: pending.
- T3 [S1,S3,S4,S5] pending — Exercise populated translation/review/validation/queue flows, prioritize discovered defects as separate units and verify each delivery. Route: inline audit; independent verification for high-risk changes; commit: pending.
- T4 [S2,S3,S6] pending — Publish final screen/system coverage, remaining product gaps, delivery SHAs and retained-evidence/cleanup report. Route: inline docs; commit: pending.

## Log
L1 (2026-10-16), original request:
> ▎ analiza el codigo, las sesiones, levanta la app, haz capturas de pantalla de cada paso, y como optimizarla por completo, dime que
> ▎ pantallas ya tengo, cuales fallan, cuales se pueden mejorar, cuales se faltan por desarroollar, en mecanicas, sistemas, etc y haz
> ▎ una lista de tareas que seguiras y haras en todo el dia, para dejarlo solo, e ir subiendo todo a main, cada tarea cuando pase al
> ▎ 100% y los test no den errores, ni de codigo ni visual, pasar a la siguiente tarea automaticamente en cadnea hasta terminar todas
> ▎ las tareas, y si salen bugs o fallos entre medio priorizarlas y resolverlas antes de pasar a la siguietne tarea, y que la
> ▎ navegacion sea fluida y sin compliaciones, que siemrre se tenga un orde, e ir borrando los test y cache viejos en cad anuevo test
> ▎ para no llenar de basuraa el disco en cada test y despues no borraarlo

L2: Baseline main af3bc33, clean; origin Miike0303/Locust, default main. Previous session summary confirms CI 38838876938 succeeded, not rechecked yet. Existing audit/fe sibling worktrees preserved. RDD off. Assumptions: isolated synthetic fixtures and mock provider only; do not alter real games, credentials or external projects. Cleanup means generated scratch/cache, never regression source tests or retained proof. Native Tauri and headless browser evidence must be distinguished. New capabilities require demonstrated gaps, not speculative expansion. Forecast: audit/docs ~250 lines, each fix ~100–300 lines; deliver sequential independent units, not an accumulated feature push.
L3: Read-only explorer maps five routes and six settings sections; no current failure proven. Reproduction targets: unknown routes, narrow layouts, backend-offline feedback and modal/job continuity. Existing historical browser harnesses need fresh paths and cannot be rerun unchanged.
L4: Baseline preparation passed: 516 frontend tests (0 failures/skips), TypeScript/Vite build and cargo build -p locust-cli; logs in tmp/desktop-flow-audit/build. Cleanup before/after exited 0, removed 0 GB; checkout target 2.85 GB, shared target 3.89 GB. Cleanup-only TEMP/TMP confined to owned audit scratch; official worktree prune authorized, unrelated worktrees preserved. Initial verifier request was blocked before execution by contradictory parent constraints, then clarified; no failing command was retried. Build warnings: aged Browserslist data, mixed api.ts imports, 506.51 kB chunk.
L5: Python on PATH is 3.14 without Playwright; existing Playwright CLI belongs to Python 3.13. Explicit py -3.13 Chromium smoke launch passed (145.0.7632.6); no dependencies installed. Fresh tmp/desktop-flow-audit/audit.py starts and owns isolated real backend/static UI processes, synthetic RenPy fixture and screenshots; job-1 running, results pending. No native Tauri evidence yet.
L6: Baseline job-1 exit 0: 23 screenshots and no page exceptions, retained at tmp/desktop-flow-audit/run-1792169214788510600. Unknown URL has empty body and no main/sidebar, confirmed visually. T2 prioritized before completing T1 under S4. At 640px/18px Editor remains navigable but cramped; no horizontal document overflow. Settings history without project was captured loading with an activity error; final state needs targeted reproduction. Pillow contact-sheet tool unavailable (ModuleNotFoundError); individual PNGs inspected directly, no dependency added.
L7: T2 adds localized NotFound recovery inside the existing shell, preserving five route destinations. RED: 1 passed/2 failed; GREEN: 3/3 focused, 519/519 full frontend, production build passed. Corrected browser acceptance run-1792169641263380300: five captures, no page exceptions, Home and Editor recovery and nested unknown EN/ES passed; parent inspected recovery images. Earlier acceptance failed on exact Editor selector (accessible shortcut included); corrected href selector and readiness waits. Python static server emitted connection-abort during browser shutdown in passing run, retained. Rust verifier logs rust-gates/01..05 .exit.txt all read back as 0 (cleanup, fmt, strict Clippy, workspace tests, cleanup); agent aggregation confirms 2,584 passed, 0 failed, 25 ignored (not executed). Target grew from 2.85 to 21.44 GB, below 30 GB cap; cleanup removed 0 GB, it did not empty the cache. Candidate ready for work-unit commit and exact-SHA hosted CI, not main delivery. Risk: none (additive unknown-route recovery; existing routes and data operations unchanged).
