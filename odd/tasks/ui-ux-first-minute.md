# UI/UX: first-minute value and measured performance
Improve Locust's existing desktop/web UI with Claude and the installed frontend-design skill; preserve product safety and current workflows.
Branch: `feat/ui-ux-first-minute`. Baseline source: `3dfd4c2`; local reviewable commits only. Verification: frontend tests/build, isolated real-backend headless flows and before/after evidence.

## Specs
S1. "quiero que con claude, uses design, y analices y tomes capturas de actal y mejorarlr en genera todo el ui y uux"
S2. "ademas agrega esto si\n aplica 1) detectá y eliminá renders innecesarios.\n 2) reducí el bundle con lazy loading.\n 3) optimizá requests y caching.\n 4) encontrá queries lentas y problemas N+1.\n 5) optimizá imágenes, fuentes y scripts."
S3. "para esto pueden usar MCPs como Chrome DevTools para medir el rendimiento, Playwright para probar flujos reales y Sentry para detectar\n problemas en producción."
S4. "si los primeros 30 segundos son malos, la gente se va"
S5. "checklist para el onboarding de tu app:\n - una sola accion por pantalla\n - mostra el valor antes de pedir registro\n - nada de 5 pantallas de \"bienvenido\"\n - que el primer resultado lo vea en menos de 1 minuto"
S6. "revisa este onboarding como alguien apurado que nunca vio la app y decime donde abandonaria"

## Tasks
- UX1 [S1-S6] in progress; preserve previous WIP, capture current routes/first-run flows and record applicable performance evidence; route parent Git/tracking + bounded tooling worker due context backstop; commit pending.
- UX2 [S1-S6] pending; Claude design audit of screenshots and hurried-new-user onboarding, compact visual direction and evidence-ranked change slices; route installed Claude CLI + official frontend-design skill; commit pending.
- UX3 [S1-S5] pending; implement prioritized UI/UX and applicable measured performance fixes in cohesive source/tests/docs slices; route Claude, one bounded slice at a time; exact surfaces derived from UX2; commits pending.
- UX4 [S1-S6] pending; before/after measurements/screenshots, existing-flow regressions, first-result timing and accessibility/EN-ES checks; route parent functional checks and independent verifier only for named high-risk changes; commit pending.

## Log
L1. User request verbatim:
> quiero que con claude, uses design, y analices y tomes capturas de actal y mejorarlr en genera todo el ui y uux, ademas agrega esto si
>  aplica 1) detectá y eliminá renders innecesarios.
>  2) reducí el bundle con lazy loading.
>  3) optimizá requests y caching.
>  4) encontrá queries lentas y problemas N+1.
>  5) optimizá imágenes, fuentes y scripts.
>
>  para esto pueden usar MCPs como Chrome DevTools para medir el rendimiento, Playwright para probar flujos reales y Sentry para detectar
>  problemas en producción."si los primeros 30 segundos son malos, la gente se va"
>
>  checklist para el onboarding de tu app:
>  - una sola accion por pantalla
>  - mostra el valor antes de pedir registro
>  - nada de 5 pantallas de "bienvenido"
>  - que el primer resultado lo vea en menos de 1 minuto
>
>  pedile: "revisa este onboarding como alguien apurado que nunca vio la app y decime donde abandonaria"
L2. Priority supersedes the day queue, not its completed results. E7c source WIP saved non-destructively at stash commit `6cb226684d740cab7b9e3945a0c6b41373cac029` on `feat/e7-model-aware-budgets`; parent verification NOT run. Worker-only E7c evidence: focused7/core1024/0/3/fmt/strictClippy; do not claim completed. E7a91db5ff/E7b3dfd4c2/E5all slices remain completed with their owning evidence. Day queue owns deferred E7c-f and remaining engine work.
L3. Installed Claude CLI found; current parent runtime is gpt-6.1-sol, so it must not be represented as Claude. Official installed skill read: `C:/Users/Mike/.claude/plugins/marketplaces/claude-plugins-official/plugins/frontend-design/skills/frontend-design/SKILL.md`. Claude audit/implementation will use the installed CLI, no silent model substitution. Skill requires subject-specific tokens/layout, design-plan critique before code, restrained motion and screenshot self-critique. No design implementation yet.
L4. Conservative assumptions: optimize only demonstrated/applicable issues, preserve current product functions and safety, expose a single primary next action rather than removing necessary expert controls. Locust is local-first: do not invent registration or production telemetry. Existing Chrome headless/CDP and websocket-client are available; no Playwright package/MCP availability claimed. Use installed tools without dependency installation; no Sentry account/instrumentation, data disclosure, real-game modification, credentials, purchasing or publication. Synthetic projects and disposable profiles only. Record first-use timing scope; a local synthetic under-one-minute result does not certify provider/network/game performance. Prior E5 screenshots are historical, not this feature's baseline.
