# RPG Maker text-fit preview: recommend an advisory advance-width probe

**Decision:** pursue only a read-only, explicitly configured horizontal advance estimate for plain MV/MZ dialogue, not an engine-rendering simulator or automatic rewrite system. The existing character wrappers and font coverage checks are useful seams, but neither proves pixel fit. Implement nothing in this spike.

Scope: day-plan [E8, S4/S5 and L72](../../odd/tasks/day-plan-2026-10-19.md), inspected at local source `6255129` on `research/e8-rpgmaker-text-fit`. This report completes the research outcome only; a later implementation needs its own bounded task and tests.

## Evidence levels and review path

- **Measured:** one bounded Python standard-library probe of Locust's owned synthetic font tables; results below. No renderer or product test was executed.
- **Source-inferred:** behavior read from the cited repository and installed `ttf-parser` public interfaces. Existing test assertions are evidence of intended contracts, not fresh passing runs.
- **Proposed:** every MVP rule, budget and acceptance case below. None is shipped or benchmarked by this spike.

Read the inventory first, then the MVP exclusions and acceptance table. No real-game files, installed game fonts, browser, provider or native engine were used; no download, dependency change or Cargo build was needed.

## What already exists — source-inferred

| Seam | Evidence and consequence |
|---|---|
| MV/MZ message records | [`rpgmaker_mv.rs:485–588`](../../crates/formats/src/rpgmaker_mv.rs#L485-L588) merges consecutive 401/405 commands into `#msg` rows tagged `dialogue`/`scroll_text`; choices and MZ speaker names are separate rows. Show Text headers retain speaker context, not a complete font/window geometry profile. |
| Message injection | [`rpgmaker_mv.rs:1349–1411`](../../crates/formats/src/rpgmaker_mv.rs#L1349-L1411) wraps at `max(source visible length, 40)`, can grow/shrink the command run, preserves paragraph blanks and source list-marker boundaries. The floor is a heuristic in characters, not 40 pixels or a measured window width. |
| Other wrapped fields | [`rpgmaker_mv.rs:1665–1690`](../../crates/formats/src/rpgmaker_mv.rs#L1665-L1690) uses the widest multiline source count as its wrap target, with zero-width/already-within-width guards and blank-line layout handling; [`2175–2179`](../../crates/formats/src/rpgmaker_mv.rs#L2175-L2179) applies it during Replace, including Iavra packs and database descriptions. Single-line sources are unchanged by this helper, not necessarily safe on screen. |
| What “visible” means | [`rpgmaker_mv.rs:1621–1654`](../../crates/formats/src/rpgmaker_mv.rs#L1621-L1654) counts Rust `char`s after skipping broad backslash syntax/name tags. It drops dynamic substitutions and icons from this count; it does not evaluate their eventual widths. [`1781–1805`](../../crates/formats/src/rpgmaker_mv.rs#L1781-L1805) splits whitespace and leaves overlong words whole. |
| RGSS differs | [`rpgmaker_vxa.rs:1205–1221`](../../crates/formats/src/rpgmaker_vxa.rs#L1205-L1221) reuses the multiline helper; [`1390–1449`](../../crates/formats/src/rpgmaker_vxa.rs#L1390-L1449) anchors messages at XP 101 / VX Ace 401, flattens whitespace and uses the same 40-character floor. Do not assume MV/MZ paragraph behavior or renderer parity. |
| Character limits | [`models.rs:22–56`](../../crates/core/src/models.rs#L22-L56) defaults `char_limit` to none; [`validation.rs:523–535`](../../crates/core/src/validation.rs#L523-L535) counts Unicode scalars when a limit exists. Neither RPG Maker extractor file sets `char_limit` or `binary_slot` in this inspection. |
| Byte capacity is separate | [`validation.rs:74–93`](../../crates/core/src/validation.rs#L74-L93) measures encoded bytes; [`120–193`](../../crates/core/src/validation.rs#L120-L193) derives explicit/tagged physical capacities and returns none without applicable metadata. JSON serialization ([`rpgmaker_mv.rs:2211–2228`](../../crates/formats/src/rpgmaker_mv.rs#L2211-L2228)) and Marshal length prefixes ([`rpgmaker_vxa.rs:699–702`](../../crates/formats/src/rpgmaker_vxa.rs#L699-L702)) permit string growth; they do not certify display space. |
| Existing length pressure | [`protection.rs:240–255`](../../crates/core/src/translation/protection.rs#L240-L255) defines encoded-byte hints and two binary-length retries. These are not pixel-width retries and must not be silently repurposed. |
| Current editor | [`DetailPanel.tsx:115–124`](../../apps/desktop/src/components/DetailPanel.tsx#L115-L124) derives draft character count and byte capacity independently. [`176–213`](../../apps/desktop/src/components/DetailPanel.tsx#L176-L213) shows the editable draft and those counters, not an engine-font measurement. |
| Coverage and font discovery | [`font_validation.rs:10–24`](../../crates/core/src/font_validation.rs#L10-L24) bounds font reads at 64 MiB; [`99–143`](../../crates/core/src/font_validation.rs#L99-L143) parses each face and tests cmap presence. [`155–176`](../../crates/core/src/font_validation.rs#L155-L176) discovers font files, not active engine font selection/fallback order. |
| Honest font limitations | [`font_validation.rs:258–260`](../../crates/core/src/font_validation.rs#L258-L260) explicitly excludes shaping, RTL, line fit and engine selection from coverage. WOFF/WOFF2 coverage is refused at [`110–114`](../../crates/core/src/font_validation.rs#L110-L114). A zero-character coverage percentage is not positive layout evidence. |
| Font patch is not layout repair | [`font_patch.rs:1–3`](../../crates/core/src/font_patch.rs#L1-L3), [`50–55`](../../crates/core/src/font_patch.rs#L50-L55), [`87–92`](../../crates/core/src/font_patch.rs#L87-L92) support an explicitly selected loose MV/MZ font asset, not RGSS configuration, engine selection, SDF or window rewrites. |

**Public metric interfaces:** core already declares [`ttf-parser = "0.21"`](../../crates/core/Cargo.toml#L26), locked to [`0.21.1`](../../Cargo.lock#L6682-L6685). The installed SDK's public `Face::units_per_em`, `glyph_index` and `glyph_hor_advance` were read at `src/lib.rs:1623–1629,1778–1799,1848–1875` ([versioned source locator](https://github.com/harfbuzz/ttf-parser/blob/v0.21.1/src/lib.rs)). `glyph_index` may return none; advance may return none and depends on variation axes. These APIs provide per-glyph metrics, not shaped text runs. The external URL is a locator, not a network-verified fetch in this spike.

**Source-inferred formula:** for a supported line and selected static face, `sum(advance(glyph(c))) * font_size_px / units_per_em` gives unkerned horizontal advance. Preserve fractional values. Advance is not ink bounds, glyph placement, kerning/GPOS, ligature substitution, bidi order, fallback resolution, hinting or engine drawing behavior. No renderer-equivalence claim follows from these APIs.

## Owned measurement — measured, not rendered

Command: `python -B .test-tmp/e8-text-fit-research/run_probe.py` (foreground, 30-second tool limit; child timeout 20 seconds).
Child command: `python -B .test-tmp/e8-text-fit-research/probe_metrics.py`.
Observed: exit **0**, stderr empty, elapsed **0.0869 s** for process startup plus this tiny table probe; this is not a preview performance benchmark.
Retained [receipt](../../.test-tmp/e8-text-fit-research/probe-receipt.json), [stdout](../../.test-tmp/e8-text-fit-research/probe.stdout.txt), [stderr](../../.test-tmp/e8-text-fit-research/probe.stderr.txt), and [probe source](../../.test-tmp/e8-text-fit-research/probe_metrics.py) are owned scratch, not guaranteed published artifacts.

The probe read only [`synthetic-ascii.ttf`](../../crates/cli/tests/fixtures/synthetic-ascii.ttf), SHA-256 `932744a7ad2237c9c2814ae99c2d09fa8bd6314923da546929cb5723ef200755`. It checked the fixture-specific cmap layout and metric tables using `struct`, not `ttf-parser`, a shaper or a general-purpose font reader.

| Observed fixture property/sample | Result |
|---|---|
| Units per em / horizontal metric entries | 1000 / 96; every advance is 500 units |
| At chosen **28 px** size | Empty string 0; space 14; `iii` 42; `WWW` 42; `AV` 28; `Hola` 56 advance-pixels |
| Missing cmap samples | `é` U+00E9, `あ` U+3042, emoji U+1F600: missing; no numeric width returned |
| Rendering fitness | No usable outlines; only ASCII cmap/metrics, per [`fixtures/README.txt:1–4`](../../crates/cli/tests/fixtures/README.txt#L1-L4) |

**Implication:** this fixture can anchor arithmetic and missing-glyph tests but cannot distinguish narrow/wide letters, kerning, accents or visible clipping. Equal `iii`/`WWW` here does not show that character counts work for real fonts. A proportional synthetic fixture is still needed. The selected 28 px size is an experiment input, not an RPG Maker default.

## Narrow MVP — proposed, not implemented

1. Admit only MV/MZ extractor-owned `dialogue` message rows; format provenance plus row kind, not a `.json` filename guess. Exclude scrolling text, choices, names, descriptions, packs, plugin text, RGSS and LCF initially.
2. Require an explicit single static TTF/OTF face, positive finite font size and positive finite **usable content width** in the same chosen pixel units. No guessed default font, system font scan or width inferred from source length. A filename is not proof the game uses that face.
3. Inspect source and current draft **as typed**, line by line; preserve empty lines, spaces and original text. Do not wrap, shorten, normalize, save, inject or alter validation status. Label clearly: “Advance estimate for these lines; injection may rewrap them.” No claim about the final injected artifact. Source and draft get independent availability results: an unsupported CJK source must not veto a supported Latin draft.
4. First supported text subset: plain printable ASCII and precomposed Latin letters/punctuation present in the selected face, with actual LF/CRLF line separators. Defer combining sequences, bidi/complex scripts, emoji/variation selectors, tabs, inline markup and all backslash controls. Unknown or malformed syntax produces unsupported, never silently stripped zero width.
5. Show per-line unkerned advance and the configured width. Use “estimated advance within chosen width” / “estimated advance exceeds chosen width”, not “fits in game”. Unknown font/glyph/control state yields unavailable with a reason, not a green result or zero-width substitution.
6. Keep character/encoded-byte checks independent. Explicitly state that horizontal advance does not certify ink clipping, vertical capacity, page breaks, font coverage as rendered, engine compatibility or linguistic quality.

This is useful as a manual warning for plain translated dialogue under a declared font/width profile. Its value is intentionally smaller than an engine preview; refusing unsupported rows is preferable to false confidence. No speculative endpoint, UI framework, persistence schema or rendering architecture is selected here.

## Engine/control uncertainty budget

**Source-inferred boundary:** the placeholder scanner recognizes a narrower vocabulary than `visible_len` ([`placeholder.rs:553–608`](../../crates/core/src/placeholder.rs#L553-L608)); placeholder preservation is not control evaluation. No MV/MZ JavaScript or RGSS runtime implementation was inspected. The interpretations below are investigation requirements/proposed handling, not verified stock-engine specifications.

| Input or engine seam | Conservative MVP handling / proof needed before expansion |
|---|---|
| `\V[n]`, `\N[n]`, `\P[n]`, `\G` | Potential runtime text substitutions: unknown until resolved to an explicit snapshot; do not count their syntax or discard their contribution. No executing game expressions/scripts. |
| `\I[n]`, font-size changes `\{`/`\}`, plugin font/spacing codes | Potential icon or style/size-dependent advance: unsupported. A future profile needs verified icon stride, size transitions and reset semantics; cmap cannot supply them. |
| `\C[n]`, `\.`, `\|`, `\!`, `\>`, `\<`, `\^`, `\$` | Candidate color/timing/window controls, not a proven zero-width whitelist here. Initially unsupported; future token classes require exact engine-version evidence and state-transition tests. |
| `\\`, `\n<Name>`, MZ header speaker | Escaped literal vs plugin namebox vs independent speaker field must remain distinct. No blanket regex removal; preserve spans. Scanner/name-tag support in Locust is not universal plugin support. |
| Face portrait, padding, indentation, namebox, window scale | Require manually supplied effective content width; no automatic deduction from event run length. Portrait reduction and resolution/theme/plugin overrides are unmeasured. |
| Font fallback / bold / italic / variable fonts | Selected face only; missing glyph => unavailable. No implicit OS/browser fallback or synthetic styling. Initially refuse collections, variable fonts and style changes rather than silently picking defaults. |
| Kerning, shaping, RTL, ligatures, marks | Unkerned label is mandatory even for Latin pairs like `AV`; complex text unavailable. A legacy kern-pair adjustment alone would still not prove complete shaping or engine equivalence. |
| Line box, outlines, descenders, page height | Font ascender/descender/line-gap metrics are not engine line height. Blank lines consume layout; tall glyphs/outline overhang can clip despite small advances. Height/page simulation is deferred. |
| Engine generations | MV and MZ need separately verified profiles; XP/VX/Ace need separate RGSS evidence. Current shared wrapping helpers do not justify sharing rendering constants or semantics. |

## Cost, tokenization and execution seams — proposed

- No provider calls, LLM tokenization or budget-price changes. “Tokens” here would be text/control spans with exact byte ranges, not model tokens. The MVP can reject controls before measurement; later extensions need a bounded, single-pass scanner, never execution of plugin code.
- Reuse metric lookup capabilities, not `visible_len` as a pixel estimator or the placeholder scanner as a full interpreter. Parse a chosen font once; cache by font-content identity/face and metric settings. Cache text results by draft revision, width and profile version; do not reuse a stale row/project/font result.
- Cost model: one bounded font read/parse, then one traversal per line plus distinct-glyph lookups; avoid repeated whole-prefix measurement and whole-project font audits on each keystroke. Proposed ceiling: 16,384 input scalars per request, explicit refusal beyond it; reuse the existing 64 MiB font-read ceiling or tighten later.
- Start on demand, one selected row, no background project scan. Keep file I/O/parsing and long measurements off the interaction-critical thread where the chosen host permits it; cancellation/latest-request ownership must prevent late results replacing newer drafts. No worker framework is prescribed.
- Neither latency nor memory at those limits was measured. Before choosing debounce/off-thread thresholds, record cold/warm font parse and 1/4/16k-scalar samples, repeated glyphs vs distinct glyphs, cache size and cancellation behavior. Proposed performance acceptance: bounded work, no stale display and retained timings, not an invented milliseconds guarantee.

## Fixture-based acceptance for a later implementation — proposed

Existing synthetic sources are useful starting points, **not tests run by this report**:

- [`rpgmaker_revision.rs:23–80`](../../crates/formats/tests/rpgmaker_revision.rs#L23-L80): generated JSON/JSONO, www/data variants, 401 dialogue, choices, plugin text, multiline profiles and packs; useful admission/exclusion cases.
- [`rpgmaker_vxa_revision.rs:43–79`](../../crates/formats/tests/rpgmaker_vxa_revision.rs#L43-L79): XP vs Ace anchors; [`rpgmaker_marshal_lossless.rs:1–95`](../../crates/formats/tests/rpgmaker_marshal_lossless.rs#L1-L95): independent wire builder. These remain excluded-engine negative cases initially, not RGSS visual proof.
- [`rpgmaker_mv.rs:3166–3230`](../../crates/formats/src/rpgmaker_mv.rs#L3166-L3230): controls/name tags/character-wrap assertions. Extend separate pixel-metric tests; do not reinterpret those assertions as pixel evidence.
- [`font_validation.rs:390–575`](../../crates/core/src/font_validation.rs#L390-L575): owned cmap/metric builder; [`font_tests.rs:14–39`](../../crates/cli/tests/font_tests.rs#L14-L39): CLI coverage/patch usage. The checked-in font has no outlines, so it cannot be a screenshot oracle.

| Required fixture/case | Explicit expected contract |
|---|---|
| Known fixed metrics | Reuse 1000-upem/500-unit fixture: `Hola` at 28 => 56; space => 14; empty => 0 **only after valid inputs**, plus separate “no text to assess” state. |
| Proportional metrics | New owned synthetic metrics: `i=200`, `W=900`, upem=1000, size=20 => `iii=12`, `WWW=54`; width=30 distinguishes within/exceeds despite equal scalar counts. Independent hand arithmetic is the oracle. |
| Boundary/scaling | Width 54 exact equality, 53.9 overflow; double size doubles advances; preserve fractional sums, blank/trailing lines and spaces; CRLF is one break. No trimming or word splitting. |
| No font / bad font | Unselected/unavailable/unreadable/empty/truncated/oversized font and missing metrics => unavailable, never success; enforce read bounds. Empty text cannot bypass invalid-font checks. |
| Missing glyph / fallback | Accent/CJK/emoji missing in ASCII fixture => unavailable with codepoints; refuse silent `.notdef`/OS substitution. An explicitly richer face must be tested independently, not unioned with another face's coverage. Unsupported CJK source plus supported Latin draft must retain independent results. |
| Negative text/control inputs | Every listed backslash class, malformed brackets, literal backslash, tabs, name tags, combining marks, bidi/ZWJ/variation selectors, plugin markup => explicit unsupported without text loss or misleading partial totals. |
| Engine/row admission | 401 plain dialogue admitted only with MV/MZ provenance; 405, choices, speaker, pack, description, RGSS and unknown provenance rejected. Do not infer geometry from a matching filename. |
| Geometry/style negatives | Zero/negative/nonfinite size/width, unsupported collection face/variation/style => unavailable; warn about unmeasured line box/portrait/ink even when horizontal advance is within width. |
| Read-only / stale work | Snapshot source/draft/database/status before and after; byte-exact unchanged. Changing row/project/font/size/width/draft while pending cannot reveal an old result; cancel/unmount releases owned work. |
| Resource/performance | 16k cap, over-limit input, repeated and distinct glyphs, rapid cancellation, cold/warm caches: deterministic bounded outcomes and retained timings; no unbounded rescans or per-keystroke disk reads. |

After arithmetic/control admission passes, any claim stronger than “advance estimate” needs an independently rendered, licensed font fixture with outlines and an exact engine/version/profile comparison. Record actual selected fallback, kerning/shaping behavior, font size, content rectangle, line height, portrait/namebox and screenshot differences. Real-game/runtime certification, automatic wrapping, shortening and writeback remain outside this recommendation.

## Spike disposition and verification boundary

- **S4:** the required feature-before-code investigation is documented here; no product code, refactor, new dependency, backend/frontend API or runtime implementation was written.
- **S5:** the pre-authorized research and reversible owned probe proceeded without renewed permission prompts. S5 does not turn the research-only E8 scope into implementation authorization.
- **RED/GREEN exception:** passive research/report plus a fixture-table inspection changes no product behavior; there is no meaningful failing product behavior test to add. The observed probe assertions are ordinary structural measurement, not a claimed TDD cycle.
- Product tests/builds were intentionally not run: they would not establish pixel fit for an unimplemented feature. Writer readback, `python -B .test-tmp/e8-text-fit-research/verify_report.py` (119 lines, 41 local links/ranges valid; one external locator not fetched) and `git diff --check` passed. The structural script also checks report whitespace because Git's ordinary diff excludes this new untracked file. Parent owns ledger/index, memory, cleanup, review disposition and Git actions.
- **Unmet proof:** no real font rendering, font fallback resolution, engine controls evaluation, kerning/shaping, vertical/page fit, post-injection pixels or performance-at-limit evidence. Completing E8 research does not complete a text-fit feature.
