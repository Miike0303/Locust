# UI/UX first-minute audit: partial evidence, not an implemented redesign

**Status:** Claude Sonnet completed a read-only frontend-design audit (exit 0, 147.19 s). Only the Spanish empty home was captured. Full-route screenshots, first-result timing and before/after verification are unavailable; no product source was changed.

- [Actual baseline home, Spanish, 1440 × 1000](evidence/ui-ux-first-minute/baseline-home-es.png)
- [Owning scope, failures and continuation decision](../odd/tasks/ui-ux-first-minute.md)
- Source baseline: `3dfd4c2`; setup-only commit: `dcc766c`.
- Retained raw audit: `C:/Users/Mike/AppData/Local/Temp/gpGBgu9W/locust-owned-claude-ui-audit-z9po9usu/`; capture evidence: `C:/Users/Mike/AppData/Local/Temp/locust-owned-ux1-07kra5zs/`.

## Parent readback and corrections

1. The screenshot already has **one visually primary green button**; Claude's “5 equal-weight actions” is imprecise. Five operations are simultaneously visible, but they do not have equal visual weight. The usability recommendation is fewer initial decisions and progressive disclosure, not a claim that all controls look identical.
2. Claude's “Evalor palette per org default” has no basis in the user's brief or repository evidence. It is **not an adopted project convention**. Any later palette must derive from Locust's actual existing tokens and verified accessibility, not this unsupported brand assumption.
3. “Five cold runs” is Claude's proposed measurement protocol, **not a new user requirement or an authorized verification loop**. No first-result time was achieved. An illustrative translation strip would not prove extraction or translation success.
4. The recorded raw network log supports the parent's 19-request count; Claude explicitly did not reconcile that count using its narrower ResourceTiming view. The duplicate config GETs and byte totals are corroborated. No render or SQL/N+1 defect is established.
5. The remaining pages were not visually inspected; several were not source-inspected either. This report is **not a whole-application audit**. Suggested copy, navigation and external-link changes are proposals, not implemented or independently verified behavior.
6. Native Tauri behavior, real-game/runtime safety, live-provider performance, production telemetry, human usability and EN rendering remain outside the observed proof. Experimental badges, all existing entry points and patch/recovery safety must survive any later change.

## Claude report (verbatim)

# Locust first-minute onboarding review (UX2, read-only)

Baseline source is `3dfd4c2`; HEAD `dcc766c` is tracking only. I made no edits and ran no commands. I used 18 read/search calls.

## 1. Evidence and limitations

**Observed (one screen only)**
- The real Spanish empty home at 1440x1000, headless Chrome 155, synthetic Ren'Py backend (`failure.png`, `failure.dom.json`, `metrics.json`).
- Cold local first paint and first contentful paint were both 136 ms. `metrics.json` records the navigation only as loaded at 90 ms.
- The final-snapshot resource list holds 13 entries plus the navigation. That does not reconcile with the supplied "19 requests", so I treat 19 as parent-supplied and unverified in the files I read.
- The five API GETs are confirmed: `/api/config` twice, `/api/project/current` (404), `/api/formats` and `/api/providers`. `favicon.ico` also returned 404.
- No fonts and no images were loaded (`fonts: []`, `images: []`).
- Home JS decodes to 592,666 B. That equals index 528,520 + PatchModal 34,573 + Welcome 22,307 + InjectionRecoveryModal 5,468 + workflowGuide 937 + plus 861, so the sum reconciles.
- Dist totals reconcile with `dist-sizes.json`: 908,755 B raw and 258,830 B gzip.
- Gzip figures are per-file in-memory compression. The SPA server sent `contentEncoding: ""`, so delivered bytes equal raw bytes in this run.

**Not measured**
- No first-game open, first-result timing, EN screenshots, populated routes, warm requests, React profiling or SQL/N+1 data exists.
- The helper failed at `ui-baseline-capture.py:307` waiting for English `'Open Game Folder'` on a Spanish page. That is a helper defect, not a product defect.
- The `clock_ms` of 8374 in the DOM json is the failure-snapshot time, not a load metric.

**Not read:** Review, Qa, Speakers, TranslationMemory and Settings source; `GameSourceDialog`, `ResumeProjectDialog`, `UpdateChecker`; the locale catalogs; the `:root` token values in `index.css`. Editor and Dashboard were only grepped.

## 2. First-30-second walkthrough (hurried stranger)

Likelihoods are my judgement, not measurements.

| t | What they see | Evidence | Abandonment risk |
|---|---|---|---|
| 0-1 s | Fast paint. | 136 ms FCP, observed. | None. |
| 1-5 s | Globe, "Project Locust", "Herramienta universal de localización de scripts". It never says who it is for or what outcome to expect. | `Welcome.tsx:371-377`; screenshot. | "Universal" is contradicted below by 12 of 15 formats tagged EXPERIMENTAL; only Ren'Py, RPG MV/MZ and RPG VX are stable (DOM). |
| 5-10 s | A 3-step "¿Nuevo en Locust?" card, then 5 equal-weight actions. | `Welcome.tsx:379-436, 439-491`; DOM buttons. | Violates S5's one action per screen. "Aplicar un parche" and "Recuperar inserción" are not first-run actions. "Abrir base del proyecto" assumes knowledge of `.locust.db`. |
| 10-30 s | **A1: no game at hand.** The only forward path is a native file picker. | `Welcome.tsx` has no sample, preview or demo affordance (full file read); DOM `inputs: []`. | Highest likelihood. Nothing shows value before a real game is chosen (S5.2). |
| 10-30 s | **A2: the 15-card format wall.** | `Welcome.tsx:751-805`; the screenshot shows it starting at y≈490. | A screen of "EXPERIMENTAL" badges reads as "unfinished". The badges are important safety disclosure, but they sit as noise instead of attached to the decision. |
| after open (source-only) | **A3: detection fails.** A 15-item modal asks the user to guess the engine. | `Welcome.tsx:222-227, 536-674`. | There is no recommended guess. The close button has no aria-label (`:547`). |
| after open (source-only) | **A4: no provider is ready.** The warning hint sits below the open buttons. | `Welcome.tsx:509-533`; `/api/providers` observed. | The first translation cannot complete. |
| any time (source-only) | **A5: sidebar dead ends.** 8 destinations are always active. | `Layout.tsx:32-41`; `Dashboard.tsx:53` and `Editor.tsx:542-546` show empty states. | Clicking "Panel", "Editor" and similar leads to an empty state that routes back to Home. |

Smaller defects:
- Two `<h1>` elements: sidebar `Layout.tsx:68` and main `Welcome.tsx:373` (DOM headings list "Project Locust" twice).
- The footer GitHub link lacks `target`/`rel` (`Welcome.tsx:816`). In a Tauri webview that may navigate away from the app (not verified).
- The uppercase label (`:753`) and 14 per-engine rainbow colors (`:92-114`) carry no information. Stability, which is the real signal, is only a small badge.

## 3. Workflow review

| Page | Basis | Finding |
|---|---|---|
| Home `/` | Observed (screenshot, DOM) plus full source | See section 2. Auto-detect, recents (`:677`), queue add and the picker fallback all exist and must be preserved. |
| Open/extract | Source-only | The `opening` state shows only a text line (`:502-506`), with no progress or ETA. Success goes straight to `/editor` (`:219`). |
| Editor | Source-only, grep only | The translate button is at `:413`. The no-project empty state links to `/` (`:542-546`). The post-open "next action" behavior is unverified. |
| Dashboard | Source-only | Five parallel independent queries (`Dashboard.tsx:38-43`) are gated on a project. There is no evidence of N+1. |
| Review, Qa, Speakers, Memory, Settings | Not inspected | Route wiring (`App.tsx:43-51`) and chunk sizes only. |
| Patch/Recovery modals | Source-only | `PatchModal` and `InjectionRecoveryModal` load on Home though only used on click (observed fetches at ≈101 ms). "Recuperar inserción" opens with an empty path when no project is open (`:487`). |
| Backend down | Source-only | Home ignores query errors (`:127-138`), so the format grid and provider hint silently vanish. |

## 4. Frontend-design plan

**Subject and job:** a local-first tool that extracts game dialogue strings, translates them and writes them back. The audience is a game fan or translator who is in a hurry. The job is to get from "I have a game folder" to "I see my first translated line".

**Tokens.** These use the Evalor palette per org default. The accent is already green, so the shift is small. `index.css` `:root` values were not read; slice 1 must map onto the existing CSS variables rather than replace them.

| Token | Hex | Use |
|---|---|---|
| mint-paper | `#F3F8F5` | background |
| ink | `#14231F` | text |
| locust-teal | `#00917F` | single primary action |
| deep-teal | `#387167` | secondary text/lines |
| leaf | `#74C697` | translated/success state |
| amber-risk | `#B45309` | experimental and patch-risk disclosure |

**Type:**
- One system UI stack, which adds zero font requests and renders CJK and accented game text.
- A mono face only for paths and extensions.
- Scale about 14/16/20/28 px, line length at most 64 characters, sentence case, no uppercase labels.

**Layout concept:** a left-aligned single column. Its most characteristic element is the "string strip": one source line becoming its translated line, tagged with the engine file (`.rpy`). It is labelled "Ejemplo ilustrativo", is static, and is not a measured result. The primary action sits directly under it.

```
rail: Inicio, Ajustes | project items muted "abra un juego primero"
+------------------------------------------------------+
| Traduzca su primer juego                             |
| [ Abrir carpeta del juego ]   ¿Es un archivo? (link) |
| e "Hello."  ->  e "Hola."        (.rpy, ejemplo)     |
| Abrir > Traducir > Escribir de vuelta                |
| v Otras formas de empezar (proyecto, parche, recup.) |
| v Formatos: 3 estables, 12 experimentales            |
+------------------------------------------------------+
```

**Principles:** spend boldness on the strip only; one primary button; the experimental badge stays on each card and in the picker, with an amber outline, icon and text; motion only on user action; keep `focus-visible` and reduced-motion.

**Self-critique, before code**
- *Cut:* the globe icon, the hero stats row, the per-engine rainbow, the uppercase label and the duplicate 3-step card (folded into the one-line sequence).
- *Kept:* numbered steps only because the sequence is real.
- *Risk:* safety copy must not claim backup behavior I did not verify. Use "Revise el resultado antes de aplicar el parche".
- *Constraint:* copy stays in the existing formal Spanish (usted).

## 5. Implementation slices

Locale catalog paths are unverified; I saw only `apps/desktop/src/lib/i18n/index.ts` and `i18n.test.ts`.

| # | Slice | Allowed paths | Tests | Preserved |
|---|---|---|---|---|
| 1 | Single-action Home: collapse secondary actions and formats into disclosures (expanded when recents or a project exist), add the strip, fix the double h1 and the GitHub link. Estimated ≤400 lines. | `apps/desktop/src/pages/Welcome.tsx`, `Welcome.test.tsx`, `components/Layout.tsx`, `lib/welcomeDesign.test.ts`, `lib/shellDesign.test.ts`, locale files under `lib/i18n/` | Welcome and design-contract vitest; typecheck; build | All 5 actions reachable, auto-detect, picker, recents, patch/recovery modals, experimental badges |
| 2 | Dedupe `/api/config`: route the boot fetch through the shared `["config"]` query | `App.tsx`, `lib/queryClient.ts`, one test | Request-count test: 1 config GET | Appearance restore, project reattach |
| 3 | Lazy-load PatchModal and InjectionRecoveryModal, and add a favicon | `Welcome.tsx`, `apps/desktop/public/` or `index.html` | Modals open on click; chunk absent from home | Modal behavior |
| 4 | Muted no-project nav plus "open a game" actions in empty states | `Layout.tsx`, `components/EmptyState.tsx`, per-page empty states | Per-page test | Alt+N hotkeys, routes |
| 5 | Backend-unreachable and provider-not-ready states on Home | `Welcome.tsx`, `lib/providerReadiness.ts` | Error-state tests | Settings deep link |
| 6 | Post-open next-action handoff in Editor | `Editor.tsx`, `components/WorkflowGuideBanner.tsx`, `lib/workflowGuide.ts` | Banner and step tests | Review/inject flow |
| 7 | Optional bundled sample project | Blocked: packaging unknown | n/a | Must be labelled "Ejemplo, no es su juego"; no provider call |

Slice 7 is a blocker because I did not read how resources are packaged. A sample may demonstrate Editor and Review. It is not provider certification and not a real-game result.

## 6. Performance

| Item | Status |
|---|---|
| Unnecessary renders | Not measured. `Layout.tsx:50-55` reads whole stores without selectors, which is a source-only candidate with unquantified impact. |
| Lazy loading | Observed: the 8 page routes are lazy and the Editor chunk (158,100 B) is not fetched on Home. Observed: the two modals are fetched eagerly (40,041 B raw, 10,400 B gzip, about 6.8% of home JS). The 528,520 B index chunk has no attribution measured. |
| Requests and caching | Observed: duplicate `/api/config` (`App.tsx:25` versus `Layout.tsx:46`). Source: `staleTime` 30 s, `retry` 1 (`queryClient.ts:4`). Warm requests and HTTP cache headers not measured. |
| Queries and N+1 | Not measured and not demonstrated. Endpoint timings are not SQL timings and no SQL profiler was identified. |
| Images | Not applicable: none loaded; icons are inline lucide. `favicon.ico` 404 observed. |
| Fonts | Not applicable: none loaded; no `@font-face` or `font-family` match in `index.css`. |
| Scripts | Observed: one module script. Compressed server delivery not observed. |

## 7. One-minute first-result acceptance protocol (no achieved claim)

- **T0, illustrative:** the static strip. This is not a result.
- **T1, sample:** a local sample project explored with no network. This is exploration only.
- **T2, real synthetic game with a configured provider:** only this tier can support a first-translation claim. The existing `renpy-200` and `renpy-2000` projects can serve as inputs.

**Procedure:**
1. Start from a cold, disposable profile.
2. The clock starts at navigation.
3. Open the folder via the real picker (moderated, human) or via an automated path injection. Report the two separately.
4. The clock stops when the first translated string is visible in the Editor.
5. Record the sub-timings: understand and click, picker, extract-to-Editor, provider call.
6. If no provider is ready, report the stop as "first extracted strings", never as a translation.

The target is under 60 s total, with at least 5 cold runs reported individually.

## 8. S1-S6 verdicts

| Spec | Verdict | Missing proof |
|---|---|---|
| S1 | Partial: one ES screenshot, plan delivered. | Other routes and EN screenshots. |
| S2 | Partial: lazy loading, duplicate request and no fonts/images observed. | Render profiling, SQL/N+1, bundle attribution. |
| S3 | Partial: CDP-style cold metrics only. | Playwright is not claimed; Sentry is not applicable to local-first. |
| S4 | Source-inferred walkthrough. | Timed human run. |
| S5 | One action per screen: fails (observed). Value before registration: no registration exists, but no value is shown before opening a game. No 5 welcome screens: passes. First result under 1 minute: unproven. | T2 timing. |
| S6 | Answered in section 2; ranked A1 to A5 are hypotheses. | Moderated run. |

**Blockers:**
- The capture helper needs a language-agnostic wait.
- The sample-project packaging (slice 7) is unknown.
- Safety copy needs verification against backend backup and recovery behavior.