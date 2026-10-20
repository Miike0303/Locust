# UI/UX first-minute audit: full-scope Claude review

**Status:** Claude Opus completed a read-only `frontend-design` audit (job-27, exit 0, 292 s, 65 turns) over the complete baseline: 36 EN/ES screenshots, metrics and source. No product code has changed yet. This supersedes the partial review in [UI-UX-FIRST-MINUTE-AUDIT.md](UI-UX-FIRST-MINUTE-AUDIT.md), which stays as history.

- Baseline: job-26 exit 0, `C:/Users/Mike/AppData/Local/Temp/locust-owned-ux1-_yhlv87h/` (retained with `.keep`); scope and failures in [the owning ledger](../odd/tasks/ui-ux-first-minute.md) L12-L14.
- Raw audit: `C:/Users/Mike/AppData/Local/Temp/gpGBgu9W/locust-owned-claude-ui-audit2-m7uhv47m/`.
- Synthetic headless only: no human trial, no translation/injection/patch run, no native Tauri dialogs.

## Parent readback

**Confirmed against screenshots or source**
- "Google Translate (free) (free)" in `07-translation-modal-en.png`; `formatProviderOptionLabel` appends the free suffix (`apps/desktop/src/lib/providerReadiness.ts:72-79`) to a name that already contains it.
- Two solid green Translate controls in one view ("Translate" toolbar, "Translate strings" banner) in `04` and `07`.
- `EmptyState` root uses `h-full flex-1` (`components/EmptyState.tsx:15`), so sibling links rendered after it land far below the message.
- "Go to Welcome" label (`lib/i18n/en.ts:426`) while the navigation says "Home".
- Boot `getConfig()` outside React Query (`App.tsx:25`) explains the duplicate cold `/api/config` GET; `PatchModal` and `InjectionRecoveryModal` are static imports on Home (`pages/Welcome.tsx:36-37`).

**Corrected**
- The "Show source column" hint (`en.ts:560-561`) describes when to clear the box; it is wordy, not contradictory. Treat as optional copy polish.

**Not independently verified**
- Blue "Translated" status colour, eager Editor modal imports and their share of the Editor chunk, Layout store re-renders (no React Profiler run), Review/QA warm re-requests root cause, Inject being enabled with zero translations at the backend level. Each must be confirmed in its slice before changing behaviour.
- "6 translated" vs "No translation runs yet" on Dashboard is a seeding artefact (rows patched through the API), as Claude notes.

## Claude report (verbatim)

# Locust first-minute UI/UX audit: Claude frontend-design review (read-only)

**Short answer to "where would a hurried newcomer give up?"**

1. **On Home, if they don't have a game folder ready.** Nothing on the screen shows what the app produces, and a wall of 15 format cards follows.
2. **When they explore the sidebar before opening a game.** Six screens show "No project open", each with a different (or missing) way back.
3. **In the Editor, with two green buttons that do the same thing.**
4. **(Returning user) On Home, which has no "continue" action.**

The first result itself is fast: extracted strings were visible 1.4 s after navigation. That was an automated run and no translation step was run.

## 1. Evidence and limits

**What I reviewed**
- All 36 screenshots (EN and ES).
- `metrics.json`: the onboarding, route_timings, endpoint_latency, skipped and limits sections.
- 20 source reads/greps on `apps/desktop/src`. The budget was 25.
- I followed the frontend-design skill process: subject, tokens, layout, critique.

**Measured facts I rely on**
- **First result (automated):** extracted strings visible 1405 ms after navigation and 428 ms after the Open action. It took 2 primary actions. In browser mode the second action is `window.prompt("Enter game folder path:")`.
- **Cold empty Home:** 19 requests, with `/api/config` fetched twice.
- **Whole session:** config fetched 7 times, project/current 5 times, strings 8 times.
- **Endpoints at 2000 rows:** strings 8.7 ms, speakers 26.8 ms (3.2 ms at 200 rows). Opening the 2000-row project took 79 ms.
- **Assets:** no fonts and no images loaded; favicon returns 404; no uncaught exceptions.

**Limits**
- This was not a human trial, and no translation, injection or patch was executed. Modals were captured as previews only.
- Translated/QA rows were seeded through the API (`metrics.json` limits). So "6 translated" next to "No translation runs yet" (11-dashboard) is a seeding artifact, not a confirmed bug.
- `route_timings` include roughly 280 ms of helper sleeps. They are not user-perceived latency.
- React re-renders, SQL timing and native Tauri dialogs were not measured.
- Anything below marked **(src)** is inferred from source and not seen in a screenshot.

## 2. Hurried-newcomer walkthrough: where they would leave

| # | Moment | What they see | Evidence | Risk |
|---|---|---|---|---|
| A0 | 0–5 s | "Project Locust / LOCalization Universal Scripting Tool". The acronym never says "translate a game's dialogue". | 01, 03; `Welcome.tsx:371-377`, `en.ts:299` | Medium: "is this for me?" |
| A1 | 5–15 s | A dismissible 3-step guide whose step 1 repeats the button below it. Then one green primary, four outlined buttons, and an orphan "Recover insertion" centered on its own row, plus a "Choose format manually" link. That is 6 choices. "Open project database" and "Recover insertion" are not first-run actions. | 01; `Welcome.tsx:379-436, 439-501` | Medium. Someone holding a game folder will find the green button. |
| **A2** | 10–30 s | **No game at hand.** There is no way to see value without your own game. The rest of the screen is 15 format cards, 12 of them labelled EXPERIMENTAL. "Universal" in the tagline reads as overclaimed. | 01, 05; `Welcome.tsx:751-805` | **Highest** for the curious or no-game visitor. |
| A3 | Pick | Browser mode shows a bare text prompt for a path (desktop uses the native dialog, which I did not observe). | `metrics.json` actual_browser_dialogs | Medium in web mode; not applicable to desktop. |
| **A4** | First result | Good: 200 real strings appear in about 1.4 s. Bad, all in the same view: **two green buttons with the same effect**, labelled "Translate" (toolbar) and "Translate strings" (banner); 7 toolbar actions; a row order that looks random (0008, 0098, 0099 … 0009). I did not find the cause of the ordering (fixture or sort). | 04; `Editor.tsx:407-413`, `WorkflowGuideBanner.tsx:99-105`, `en.ts:409` vs `:1210` | Medium: "which one?" |
| A5 | Translate modal | Defaults let one click work. But the provider shows **"Google Translate (free) (free)"**, and 10 controls (fallbacks, batch, parallel, cost) appear before any first run. | 07; `lib/providerReadiness.ts:78` appends "(free)" to a name that already contains it | Low–medium; hurts trust. |
| A6 | Inject modal | The language grid is cut off mid-row. The selection shows a code ("Selected: es"), not a name. "Inject Translations" stays enabled with 0 translated. Whether the backend blocks that was not verified. | 08 | Medium: risk of a confusing no-op. |
| A7 | Patch modal | With Apply disabled, the filled amber **Rollback** is the strongest-looking control. Seven controls in total. | 09 | Safety and perception. Do not change behavior, only visual weight. |
| **A8** | Sidebar exploration with no project | Six project routes are clickable, and each empty state looks different: Dashboard's link is pushed to y≈970; Editor and Review say "Go to **Welcome**" while the nav says "Home"; QA has the link under the text; **Memory has no action at all**; Speakers says "Project identity required" under two lines of jargon, with the link at y≈990; Review has no page header. | 02-*; `EmptyState.tsx:15` (`h-full flex-1` pushes sibling links down), `Dashboard.tsx:52-54`, `Speakers.tsx:116`, `TranslationMemory.tsx:114-119`, `Review.tsx:231-239`, `en.ts:18, 426` | **High**: "the app is empty or broken". |
| A9 | Returning user | With renpy-200 open, Home still leads with "Open Game Folder". The recent project sits below the buttons with two full paths and a "PROJECT DB" badge, and there is no "Continue" action. | 05, 11-home, 14-home-es; `Welcome.tsx:439-501, 677-749` | Medium. |
| A10 | Settings | The first provider card is the paid Grok, and its "Sign in" is the page's only solid button, implying you must pay. Default provider is "None". Default languages are free-text `ja`→`en`, while the Translate modal proposed English→Español. Interface language is buried at the bottom of Appearance. | 11-settings, 12-translation defaults, 12-appearance | Medium for "do I need to pay?". |

**S5 checklist verdict**

| Item | Verdict |
|---|---|
| One action per screen | Partial on Home (one primary, but 6 choices plus the guide). Fails in the Editor (2 primaries for the same action). |
| Value before registration | No registration exists, but value requires your own game. |
| No 5 welcome screens | Pass: there are none, only one dismissible card. |
| First result under 1 minute | Pass for *extracted strings* (automated, synthetic run). Translation was not measured. |

## 3. Per-screen critique

**Home** (01, 03, 05, 11-home, 14-home-es)
- 01 and 03 are identical.
- Hierarchy: one primary, which is correct. Below it, six peers plus the guide, and a 15-card grid with 14 rainbow icon tints (`Welcome.tsx:92-114`) that carry no information. Stability, the real signal, is only a small badge.
- ALL-CAPS labels: AVAILABLE FORMATS, RECENT PROJECTS, EXPERIMENTAL.
- The Godot description is clipped ("Replace creates an…"; ES ends "…(experimental)….").
- Recent row: an unlabeled "+" icon (title attribute only), two raw paths, and the format id shown as a pink chip.
- ES: buttons reflow to 3+2 rows; no clipping.
- **(src)**: two `<h1>` elements (`Layout.tsx:68`, `Welcome.tsx:373`). The footer GitHub link lacks `target`/`rel` (`Welcome.tsx:816`), unlike `Layout.tsx:141-143`. The format-picker close button has no aria-label (`:548`).

**Dashboard** (02, 11, 14-es)
- Empty: the link sits about 470 px below its message.
- Populated: the "Next step" button uses the pale mint style while the Editor uses solid green, so primary emphasis is inconsistent. The "Translated" legend dot and bar segment are pale mint on white, which is visually low contrast for a non-text element (estimate, not measured). The database path is exposed. Large empty area below.
- ES fine. "$0.00" is not locale-formatted.

**Editor** (02, 04, 06–10, 11, 14-es)
- Empty: the toolbar shows 7 disabled actions above a "Go to Welcome" button.
- Populated: two primaries; "Translated" status is blue (`StringTable.tsx:33,41`), the only off-palette hue in the core flow; the Tags column repeats "dialogue" on every row and adds no information.
- Detail panel: "Entry Detail" in Title Case. The Pending/Reviewed/Approved pills have no visible selected state.
- Queue modal (10): the only visible focus ring in the whole set, which is good.
- ES: the toolbar fits.

**Review** (02, 11, 14-es)
- Empty: no page header.
- Populated: the Previous/Skip/Approve controls sit about 650 px below the content. SOURCE/TRANSLATION labels are in caps.
- **The missing `{player}` that QA flags is not signalled here** (11-review vs 11-qa).
- ES fine.

**QA** (02, 11, 14-es)
- Empty: the "Run validation" button looks enabled.
- Populated: uppercase table headers; "Open in editor" and "Ignore" have equal weight; the message is backend jargon and **stays in English in ES** ("placeholder mismatch against semantic or physical source").

**Speakers** (02, 11, 14-es)
- Two disclaimer lines read before anything else, plus the jargon empty state.
- Populated: functional. The alice, bob and unassigned counts sum to 200, which matches the fixture.

**Memory** (02, 11, 14-es)
- Empty: dead end.
- EN 11 was captured mid-load ("Loading memory…"); ES 14 shows the real empty state, so this is capture timing.
- "Clear All" stays enabled with 0 entries.

**Settings** (11, 12 ×5, 13-es)
- A third navigation column.
- "Mock (testing)" provider is visible to end users.
- The checkbox "Show source column" has the hint "Hide the original text column…", which contradicts it (`en.ts:560-561`).
- Glossary pair is free text ("ja-en").
- Data page only says "No backups found.".
- ES: "Valores predeterminados de traducción" wraps to 2 lines; acceptable.

**Cross-cutting**
- Five empty-state patterns.
- QA and Speakers lack Alt shortcuts (`Layout.tsx:37-38`).
- **(src)**: the `EmptyState` button has no focus-visible ring (`EmptyState.tsx:22`), and `index.css` has no global reduced-motion or focus rule.

## 4. Visual direction

**Subject:** a local tool that turns a game folder into translatable lines and writes them back. **Audience:** a fan or translator in a hurry. **Primary job:** go from "a game folder" to "my lines, translated", safely.

**Tokens** (existing `index.css :root` light values; no new brand):

| Role | Hex |
|---|---|
| Ink | `#111827` (`--text`) |
| Paper | `#ffffff` (`--surface`) |
| Rail / canvas | `#f9fafb` (`--surface-muted`) |
| Single primary action | `#047857` (`--accent`): white text measures 5.5:1 |
| Selection / active | `#d1fae5` (`--accent-muted`): `#047857` text on it measures about 4.8:1 |
| Risk / experimental | `#92400e` on `#fef3c7` (`--warning` / `--warning-muted`): about 6.4:1 |

- Remove the blue status color. Show translated vs approved by **shape** (hollow vs filled dot in `--accent`), not hue alone.

**Type**
- Keep the system stack. That keeps the measured 0 font requests and renders CJK and accented game text.
- Keep the existing 12/14/16/20 scale (`index.css:49-56`). Add one 28 px step used only for the Home headline.
- Sentence case everywhere; drop the uppercase labels and badges.

**The one memorable element:** placeholders such as `{player}` rendered as locked chips in the source and translation cells. That is exactly what QA catches and what breaks games. Spend the boldness there and keep everything else quiet.

**Home layout** (left-aligned single column, max about 64 characters per line):
```
Translate a game's dialogue                       (28px)
Open the game folder. Locust lists every line it can translate.
[ Open game folder ]   Open a single file
Works with Ren'Py, RPG Maker MV/MZ, RPG Maker VX Ace/VX/XP.
> 12 more engines (experimental)          <- badges kept inside
> Other ways to start: project database, apply a patch,
  recover an insertion, choose format manually
-- with recents/project: [ Continue renpy-200 ] becomes the primary --
```

**Editor layout:**
```
renpy-200  Ren'Py  [Original game]            3%  194 pending  History
[Translate 194 lines]  Validate  Inject  Patch       Export Replace Pivot
 1 Translate - 2 Review - 3 Inject   (banner: text only, no 2nd button)
filters ...
Status | Source  "Hi {player}"        | Translation          | File
```

**Principles**
- One solid-accent button per view.
- Every empty state ends in an action placed right under its message.
- Risk is disclosed next to the decision it affects (experimental formats, Rollback).
- Motion only in response to user actions.

**Self-critique**
- "Single CTA plus disclosures" is a common pattern; I keep it because S5 asks for exactly that, not for style.
- Collapsing the format grid could hide the experimental disclosure. Mitigation: the stable engines are named inline, the badges stay visible in the disclosure and in the picker.
- The placeholder chips must be display-only and must not alter text or editing. If that can't be guaranteed, drop them.
- Before handoff I removed one accessory: an illustrative "sample line" strip. It would show a result Locust didn't produce.

## 5. Performance verdicts

| Item | Status | Finding and fix |
|---|---|---|
| Unnecessary renders | Not measured; candidate from source | `Layout.tsx:50-55` subscribes to the entire log and queue stores, so the shell re-renders on every log or queue change. Fix with `useLogStore(s=>s.unreadErrors)`-style selectors. Low impact; do it only after a React Profiler capture confirms it. |
| Lazy loading / bundle | Measured (routes) / from source (modals) | Routes are already lazy (`App.tsx:12-19`). Home eagerly imports Patch and InjectionRecovery (`Welcome.tsx:36-37`, about 40 KB raw / 10.4 KB gzip), and `PatchModal` is always mounted (`:825`). The Editor eagerly imports 8 modals (`Editor.tsx:40-50`); how much of its 158 KB chunk they account for was not measured. Fix: `lazy()` the modals and mount them only when open. Modest gain for a local desktop app. |
| Requests and caching | Measured | `/api/config` is fetched twice on cold load: `App.tsx:25` calls it outside React Query, while `Layout.tsx:46` and `Welcome.tsx:131` share the `["config"]` cache. Fix: `queryClient.fetchQuery(["config"])` in `App.tsx`. Also: project/current ×5; warm revisits re-request Review ×4 and QA ×4. Review a shared current-project key and `staleTime`. Favicon 404: add an icon. |
| Slow queries / N+1 | Measured at loopback level only | No slow endpoint and no N+1 evidence: everything under 9 ms except speakers at 26.8 ms with 2000 rows, roughly linear with row count. Do nothing until a SQL plan or timing shows a cause. |
| Images / fonts / scripts | Measured | None loaded; one module script. Not applicable, except the favicon. Do not add a webfont. |

## 6. Ranked implementation slices (each ≤400 authored lines)

**1. Empty-state contract.** About 250 lines.
- Files: `components/EmptyState.tsx`, `pages/{Dashboard,Qa,Speakers,TranslationMemory,Review}.tsx`, `lib/i18n/{en,es}.ts`.
- Change:
  - `EmptyState` gets an action or link slot rendered right under the text, plus a focus-visible ring.
  - All 6 routes use it with the label "Go to Home" / "Ir al inicio" (replacing "Go to Welcome").
  - Memory gets an action; Review gets its header.
  - Speakers says "No project open", and its disclaimers are shown only with a project.
- Tests (vitest):
  - New `EmptyState.test.tsx`.
  - Extend the Dashboard, Qa, Speakers, TranslationMemory and Review tests: one link or button to "/" by role and name, in both EN and ES.

**2. Home: one decision.** About 350 lines; split into 2a (copy and disclosures) and 2b (Continue) if needed.
- Files: `pages/Welcome.tsx`, `components/Layout.tsx` (rail title becomes a non-h1), i18n, `Welcome.test.tsx`.
- Change:
  - New headline and copy; Open game folder stays primary.
  - Disclosures for the other entry points and for experimental formats; neutral icon tints; sentence case.
  - "Continue {name}" becomes the primary when a project is open.
  - Footer link gets `target`/`rel`; picker close gets an aria-label.
- Tests:
  - Exactly one primary button.
  - All 5 entry points plus "Choose format manually" reachable after expanding.
  - Experimental badge count equals the count of formats marked experimental.
  - Continue shown when the project store is set.
  - Only one h1.

**3. Editor handoff.** About 150 lines.
- Files: `pages/Editor.tsx`, `components/WorkflowGuideBanner.tsx`, `lib/providerReadiness.ts`, `components/StringTable.tsx`, i18n.
- Change:
  - While the guide is visible, only one primary-styled Translate control, with a single label.
  - No duplicated "(free)".
  - Status shown by shape instead of blue.
- Tests:
  - `providerReadiness` label test.
  - Editor renders one primary-styled control.

**4. Requests and assets.** About 120 lines.
- Files: `App.tsx`, `lib/queryClient.ts`, `pages/Welcome.tsx`, `apps/desktop/index.html` or `public/`.
- Tests:
  - Mount App: `getConfig` is called once.
  - Patch and Recovery modals are absent until clicked, then found with `findByRole`.

**5. Risky-modal hierarchy (visual only).** About 200 lines.
- Files: `components/InjectModal.tsx`, `components/PatchModal.tsx`.
- Change:
  - Warn when nothing is translated or approved.
  - Show language names instead of codes; fix the clipped grid.
  - Rollback styled as outlined, not filled.
- Tests: extend `InjectModal.test` and `PatchModal.test`, asserting that existing confirmations and dry-run paths are unchanged.

**6. Settings first-run.** About 200 lines.
- Change:
  - Mark the active or default provider.
  - Fix the contradictory show-source hint.
  - Make interface language reachable at the top of Appearance.
- Language-default mismatch (ja→en in Settings vs English→Español in the modal): flag only. Changing it changes backend config, which needs your call.

**Later:** Review shows QA issues inline (needs an API check); placeholder chips (after slices 1–3); Layout store selectors (after profiling).

**Must preserve in every slice**
- All 5 Home entry points plus manual format choice, auto-detect, recents, and add-to-queue.
- Experimental badges on the Home disclosure and in the picker.
- Patch verify/dry-run/rollback and recovery semantics.
- Alt hotkeys and the EN/ES key parity checked by `i18n.test.ts`.

## 7. Before/after acceptance protocol

Re-run the same helper (same fixture, viewport, EN then ES) and re-capture all 36 file names.

**Must change**
- **01/03:** exactly one solid-accent button; ≤3 visible start actions; no uppercase labels; the stable engines named in text.
- **02-\*:** each route has an action within about 120 px under its message; the EN label reads "Go to Home".
- **04 / 11-editor:** one solid primary.
- **07:** "(free)" appears once.
- **08:** no clipped radio row; a zero-translated warning is visible.
- **09:** Rollback is not filled while Apply is disabled.
- **05 / 11-home / 14-home-es:** "Continue renpy-200" is the primary.
- **13/14-es:** no new overflow.

**Metrics**
- Cold Home: `/api/config` GET = 1, ≤18 requests, modal chunks not fetched.
- Onboarding nav→strings stays at about 1.4 s or better; it is a single synthetic run, so report variance honestly.

**Still needed to fully answer S6:** at least one moderated, timed human run from a cold profile. It should record time to first extracted strings separately from time to the first translated string.