# Japanese VN / RPG Maker translation — process & lessons

Knowledge base for translating Japanese games (JA→EN→ES) with Locust, so future
engine support doesn't start from scratch. Covers the general pipeline, per-engine
playbooks, decryption, tools, and hard-won gotchas.

Reusable scripts live next to this file in `docs/vn-tools/`.

---

## 0. General pipeline (engine-agnostic)

```
extract text ──► Locust DB ──► translate JA→EN ──► pivot ──► translate EN→ES ──► reinject ──► pack patch ──► apply
   (per engine)   (sqlite)     (grok-sub)         (new DB)   (grok-sub)          (per engine) (locust patch) (locust apply)
```

### Locust patch pack / apply (shared by all engines)

`locust patch` packs **only from an injection recording** — not from “whatever is on
disk”. That recording is created when you inject:

```bash
# Preferred for packing: in-place inject that records the game root
locust inject "<game>" -P project.locust.db --direct -l es

# Or Replace/Add (recording root is the per-language copy / Add tree)
locust inject "<game>" -P project.locust.db -l es -o <output_dir>
```

Then package and apply without hand-unzipping:

```bash
# Prefer a pristine tree for original hashes (strict verify on apply)
locust patch "<recorded_root>" -P project.locust.db -l es -o game-es-patch.zip --pristine "<pristine_game>"

# Apply onto a clean copy of the game (never the only original without backup)
locust apply "<clean_game_copy>" game-es-patch.zip
# Or download then apply in one step (http/https only):
locust apply "<clean_game_copy>" --url "https://example.com/game-es-patch.zip"
locust patch-status "<clean_game_copy>"
locust patch-rollback "<clean_game_copy>"   # restores .locust/backup
```

- Backups and receipts live in `<game>/.locust/` (hidden on Windows).
- Server: `POST /api/patch/{verify,apply,rollback,status,pack}` (binds **127.0.0.1** by default).
- Desktop: Editor → **Inject** (Direct mode records for packing) → **Patch** (Ctrl+Shift+P)
  → **Pack** tab (same core pack as the CLI). Apply/Verify/Rollback stay on the Apply tab.
- Patch zips are **ZIP64-capable** (multi‑GB entries). **Verify/apply stream** entry bytes
  (hash/stage to disk); default uncompressed ceiling **32 GiB**
  (`LOCUST_PATCH_MAX_UNCOMPRESSED` to raise). Actual expansion past a zip header’s
  declared size aborts (zip-bomb guard).
- **Binary slot engines** (Unity UTF-8, Unreal UTF-16LE heuristic, Wolf Shift-JIS):
  inject **skips** oversize translations (no hard fail). Extract tags
  `metadata.binary_slot` (`utf8` / `utf16le` / `sjis`); `locust validate <db>` reports
  `ExceedsBinarySlot` before inject. The translation engine does **up to two length-aware
  retries** when a provider returns an oversize binary-slot string. First-pass hints use
  **HARD MAX** wording for budgets ≤12 encoded bytes; the retry quotes the failed
  attempt and exact excess so the model can edit rather than retranslate. Provider
  **fallback chains**: CLI `--fallback a,b,c` and desktop translate dialog (same core
  `run_fallback_chain`).
- The `mock` provider is length-safe for UTF-8 and UTF-16LE slots (good for inject tests).
- **Real-provider length-aware ES E2E (grok-sub en→es, Unity fixture):** after HARD MAX
  prompts + dual retries, **0 oversize** (`New Game`→`Nueva Pt`, `Load Game`→`Cargar J`,
  `Options`→`Opcns`); `locust validate` clean. Inject still skips any remaining oversize.

**Phase-2 apply / real-game notes (copies or patch paks, mock or equal-length where
needed):** RPG Maker MV, MZ, XP/VXA, Ren'Py, SugarCube, HTML generic, Unity (BOXMAN +
structural TextAsset path + grok-sub ES binary-slot fixture), Unreal (Last Hope — **8.4 GB base pak** path proven for
pack/apply tooling; locres inject writes sibling `*_LOCUST_P.pak`), Wolf RPG
(synthetic `Data/*.wolf` — no commercial title on disk yet), VNTextPatch JSON
(synthetic + Ochiru EN subset via external VNTextPatch pack). Experimental engines
(KiriKiri/YU-RIS/Tyrano/NScripter/QSP) have synthetic Locust fixtures; commercial
E2E varies — see per-engine sections.

- **Locust DB**: each string has `id`, `source`, `translation`, `status`, `file_path`.
  For VNTextPatch-format games the `id` is `<jsonname>.json#<index>#message`, which
  maps back to the source file + line for reinjection.
- **Translate**: `locust translate <db> -p grok-sub -s ja -t en --concurrency N --batch-size B --context "..."`.
  Optional `--fallback provider2,provider3`. Only `status=Pending` rows are sent, so
  re-running resumes cleanly.
- **Pivot** (bridge language): `locust pivot <ja_db> -o <en2es_db>` copies each EN
  translation as the SOURCE of a new DB → translate `-s en -t es`. Then `-s en -t fr`, etc.
- **Neutral LatAm Spanish** is applied automatically by the provider prompt for `es*`.

### Desktop app (practical)

The Tauri app talks to the same core/server as the CLI. For a full in-app loop:

1. Open project → translate (optional fallback chain in the translate dialog).
2. **Validate** (Ctrl+Shift+V) → results panel with jump-to-entry; binary-slot oversize
   listed as `ExceedsBinarySlot`.
3. **Inject** → **Direct** for packable recordings (automatic backup when the engine
   mutates the original tree); Replace/Add for copies. For RPG Maker multi-lang
   (Iavra/VisuMZ): **Register … in game UI** (same as CLI `register-lang`) from the
   Inject modal.
4. **Patch → Pack** (zip) then Apply on a clean install; or use CLI apply. Apply accepts
   a local zip **or** an http(s) **URL** (desktop Patch modal + `locust apply --url`).
5. **Settings → Glossary** for preferred terms; **Settings → History** for past runs
   (cost / tokens / duration — same ledger as `locust stats`).

### Translation gotchas (all engines)
- **Count mismatch** (`sent 20 got 19`): grok merges adjacent short lines and the
  count guard rejects the batch. The SAME batch fails every retry → permanent stall.
  **Fix: shrink batch size** on stall (20→12→6→3→2). Small batches don't merge.
  See `tonight_finish.sh` / `taimanin_translate.sh` `tr_until_done()`.
- **Cost/usage**: grok-sub is a subscription ($0/token). Runs are logged in the DB
  `translation_runs` table — `locust stats <db>` or Desktop **Settings → History**.
- Save the whole run in ONE sqlite transaction (Locust does) or 30k strings take minutes.

---

## 1. Engine playbooks

### RPG Maker MV / MZ — EASY
- Text lives in plaintext `data/*.json` (`Map*.json`, `CommonEvents.json`, etc.).
- Locust `crates/formats/rpgmaker_mv.rs` handles extract + inject directly.
- Message-block extraction merges consecutive 401/405 command runs, pulls the speaker
  from the 101 command, and re-wraps by visible width to avoid dialogue-box overflow.
- `locust extract <game>` → `locust translate` → `locust inject <game> -P <db> -l es`.

### RPG Maker VX Ace — MEDIUM
- Scripts/data in `.rvdata2` (Ruby Marshal). Locust reads/writes Marshal.
- Detect encryption wrapper `{uid,bid,data}`; `is_encrypted()` in the plugin.

### KiriKiri (KAG, `.ks` text scripts) — MEDIUM  ✅ Locust Experimental + Ochiru external
- Scripts are `.ks` text (UTF-16LE / UTF-8 / Shift-JIS; **FE FE** cipher header modes
  **0 / 1 / 2** — mode 2 is zlib-compressed UTF-16LE). Often stored in `data.xp3`.
- The game auto-loads `patch.xp3`, `patch2.xp3`, … as **overlays** over `data.xp3`
  (higher number wins). **Unencrypted** patch XP3 is enough for many titles.
- **Locust-native (synthetic + real-game loose/.xp3 where unencrypted):**
  ```bash
  locust extract "<game_or_loose_ks_tree>" -o project.locust.db
  locust translate project.locust.db -p … -s ja -t es
  locust inject "<game>" -P project.locust.db --direct -l es   # writes patch.xp3 when packing XP3
  # Or inject into loose .ks and pack with locust patch / external xp3 tools
  ```
  - **Unencrypted XP3**: read full archive; inject can emit **`patch.xp3`** (engine
    auto-loads it — no need to rewrite `data.xp3`).
  - **FE FE 0/1/2** on loose `.ks` (mode-2 zlib verified against KirikiriDescrambler).
  - **Not supported:** cxdec / encrypted content, Motto-style Hxv4 custom indexes.
- **External pipeline (Ochiru worked end-to-end — still valid for encrypted bases):**
  1. Extract `data.xp3` preserving internal paths: `garbro_extract_paths.ps1 <xp3> <out> [scheme]`.
  2. Extract dialogue: `kag_extract_recursive.py <game_dir> <vntp_dir> <positions.json>`
     — decodes (BOM utf-16 / cp932), `splitlines()`, keeps lines with Japanese after
     stripping `[tags]`, skips `@`/`*`/`;` command/label/comment lines. Writes one
     VNTextPatch `{message}` JSON per file (name = flattened relpath), plus a positions map.
  3. `locust extract <vntp_dir> -o <ja_db>` → translate JA→EN → pivot → EN→ES.
  4. Reinject: `reinject_ochiru.py` — maps flatname→real path, replaces line
     `positions[flat][i]` with translation `i`. **Writes UTF-16LE + BOM** (cp932 CANNOT
     hold Spanish á/ñ/¿¡; KiriKiri reads UTF-16 natively).
  5. Repack: `krkr-xp3` (`python xp3.py -m r <patch_tree> <patch2.xp3>`) → drop as
     `patch2.xp3` in the game folder. Non-destructive; delete to revert.
- **DO NOT translate `*labels` (jump targets — breaks the game) or `;comments` (not shown).**
  The extractor correctly skips them; residual Japanese in those lines is expected/harmless.

### KiriKiriZ (compiled `.scn`) — HARD  ⛔ Motto (pending)
- Modern KiriKiriZ compiles scenarios to `.scn` = **PSB** (M2 Packaged Struct Binary).
  Also `.psb`, `.psz`, `.mdf` (compressed PSB), `.pimg`.
- Even after XP3 decryption the `.scn` are BINARY → a text-coherence check sees garbage.
- **Tool: FreeMote** (`FreeMote.Tools.PsbDecompile`) decompiles `.scn`→JSON→editable,
  and recompiles. **VNTextPatch already bundles `FreeMote.Psb.dll`** and supports SCN
  (see the net8 fork). `KrKrZSceneManager` is an alternative `.scn` string editor.
- **Planned pipeline:** GARbro/scheme extract `scn.xp3` → FreeMote decompile `.scn`→JSON
  → VNTextPatch extract text (or KiriKiriSCNJSONParser) → translate → VNTextPatch insert
  → FreeMote recompile → repack patch xp3.
- **Motto is the hardest — 4 layers of protection (investigated, static extraction infeasible):**
  1. XP3 **v2 extended header** (`QWORD@11 == 0x17`; real index offset is a later QWORD,
     empirically at byte 32 for scn.xp3). GARbro/krkr-xp3/arc_unpacker all choke here
     ("Unknown entry").
  2. The zlib-decompressed index does NOT use standard `File`/`info`/`segm` chunks — it
     starts with a **custom `Hxv4` tag** (obfuscated/game-specific index). No standard tool parses it.
  3. Content entries have the **encryption flag set (256/257)** → CxDec on top.
  4. The `.scn` themselves are **compiled PSB** (need FreeMote after all the above).
  My own v2 parser (`parse_motto_scn.py`) reads the header + zlib index but hits the
  custom `Hxv4` format, and content is CxDec-encrypted anyway.
  **Verdict: static extraction is infeasible without deep custom RE. The realistic path
  is KrkrExtract runtime dump (§3, user runs the game), which bypasses ALL 4 layers at
  once (index, CxDec, decompression) — then FreeMote decompiles the dumped `.scn`.**
  FreeMote needs .NET 4.8. data.xp3 (1.5GB) may also hold scripts but wasn't cracked.

### YU-RIS (`.ybn` in `.ypf`) — MEDIUM  ✅ Locust Experimental + Injuu external
- DLLs `YSOHC/YSPNG/YSSNP/YSTCH/YSWBP/YSZLB`, archives `pac/*.ypf`, config `yscfg.dat`.
- **Locust-native:**
  ```bash
  locust extract "<game>" -o project.locust.db   # loose YSTB .ybn and/or YPF
  locust translate project.locust.db -p … -s ja -t es
  locust inject "<game>" -P project.locust.db --direct -l es
  ```
  - **Loose YSTB `.ybn`**: XOR key = first attr descriptor offset dword (attr+8); real-game
    validated (e.g. Injuu).
  - **YPF unpack/repack**: common versions including **0x1E4**; inject rebuilds the archive
    with a **`.locust-old`** rename backup (restore on write failure).
  - Exotic YPF encryption schemes still out of scope.
- **External (Injuu shipped orchestrator `ysbin-patch.ps1`):**
  - unpack: `YPF.exe -e ysbin.ypf` → `VNTextPatch extractlocal ysbin output` (→ `output/*.json`)
  - pack:   `VNTextPatch insertlocal ysbin output res` → merge → `YPF.exe -c ysbin -v 500`
  - Backs up `ysbin.ypf` → `.bak`. **Run with pwsh 7** (not Windows PowerShell 5.1).
  - Locust DB id `<name>.json#<i>#message` maps to `output/<name>.json[i]["message"]`
    (VNTextPatch local JSON). See `inject_injuu_json.py`.

### TyranoBuilder / TyranoScript — MEDIUM  ✅ Locust Experimental
- Scenario scripts: `data/scenario/*.ks` **UTF-8** (optional BOM preserved).
- Desktop packs:
  - **Electron**: `app.asar` (and `resources/app.asar`) — unpack scenario `.ks`, inject,
    rebuild asar with **`.locust-old`** safety rename.
  - **NW.js**: `package.nw` (plain ZIP) or **`data.exe` / game `.exe`** with ZIP **appended**
    after the PE stub — Locust finds EOCD from the tail, preserves the exe prefix on rebuild.
- ```bash
  locust extract "<game>" -o project.locust.db
  locust translate project.locust.db -p … -s ja -t es
  locust inject "<game>" -P project.locust.db --direct -l es
  ```
- Registered **before** KiriKiri so Tyrano `data/scenario/*.ks` is not claimed as KAG.
- Synthetic fixtures only for containers; no commercial Tyrano E2E claimed yet.

### NScripter / ONScripter — MEDIUM  ✅ Locust Experimental
- Engine read priority (ONScripter-compatible): **`0.txt` > `00.txt` > `nscr_sec.dat` >
  `nscript.dat`**. Do **not** prefer a leftover `nscript.___` over real scripts.
- Encodings: Shift-JIS dialogue lines (heuristic: lead byte ≥ 0x80 or backtick).
- Containers:
  - Plaintext `0.txt` / `00.txt`
  - **`nscript.dat`**: whole-file XOR **0x84**
  - **`nscr_sec.dat`**: ONScripter encrypt_mode 2 — rotating XOR magic
    `{0x79,0x57,0x0D,0x80,0x04}` (self-inverse)
- ```bash
  locust extract "<game>" -o project.locust.db
  locust translate … ; locust inject … --direct -l es
  ```
- Synthetic fixtures; no NSA archives / `nscript.___` support.

### Unity — MEDIUM  ✅ Locust (heuristic + SerializedFile slices 1–2)
- **VN text scripts** under `*_Data/SCRIPTS~` (etc.) when present — preferred path.
- **SerializedFile** (format versions **17–22**) under `*_Data`:
  - Files: `*.assets`, extensionless `globalgamemanagers` / `resources` / `level*`.
  - Structural **TextAsset** (class 49): `m_Name` + `m_Script`; ids `textasset/<path_id>`.
  - Structural **MonoBehaviour** (class **114 or negative** script-type ids): `m_Name` +
    sequential aligned-string fields (skips up to 16 implausible 4-byte non-string words
    between strings); ids `monobehaviour/<path_id>/<field_index>`.
  - Structural **TextMesh** (class **141**): `m_Text` after `m_GameObject` PPtr; ids
    `textmesh/<path_id>`.
  - Structural **GUIText** (class **132**): Behaviour base + `m_PixelOffset` then `m_Text`;
    ids `guitext/<path_id>`.
  - Type-tree **blobs skipped** (object table still reachable); full type-tree field walk /
    object-table rewrite: **not** yet.
  - Inject structural: **same-or-shorter** in place (pad `0x20`); length-prefix u32 left
    **byte-identical** (endian-safe). Longer → skip + length-aware retry / `ExceedsBinarySlot`.
  - Parse fail falls back to pure heuristic scan.
- **Heuristic** length-prefixed UTF-8 on the same files (skips structural object ranges).
  Supports LE/BE length prefixes (`metadata.length_endian`); inject ≤ source UTF-8 bytes.
- ```bash
  locust extract "<game>" -o project.locust.db
  locust validate project.locust.db          # binary slots
  locust translate … ; locust inject … --direct -l es
  locust patch … ; locust apply …
  ```

### Unreal Engine — MEDIUM–HARD  ✅ Locust Experimental + Last Hope path
- **Structural `.locres`** (Localization culture files):
  - Extract from **loose** `Content/Localization/.../*.locres` and **embedded** blobs
    inside `.pak` (per-culture ids so cultures do not collide).
  - Inject loose locres: rewrite file in place (variable length OK).
  - Inject **embedded** locres: does **not** rewrite the multi‑GB base pak — builds a
    sibling **`<base_stem>_LOCUST_P.pak`** (uncompressed patch pak). UE mounts `*_P.pak`
    over the base by name ordering. Base file stays intact.
- **Heuristic UTF-16LE** still scans paks for other strings; inject uses in-place slots
  (≤ source UTF-16LE) via **direct** inject / AC multi-pattern search.
- Pack/apply tooling proven on **~8.4 GB** base pak paths; patch zips ZIP64 + streaming
  verify/apply (see §0).
- ```bash
  locust extract "<game>" -o project.locust.db
  locust translate … ; locust inject "<game>" -P project.locust.db --direct -l es
  # Expect …_LOCUST_P.pak next to the base pak when locres was embedded
  locust patch "<recorded_root>" -P project.locust.db -l es -o patch.zip
  locust apply "<clean_install>" patch.zip
  ```

---

## 2. XP3 decryption (CxDec) — the CRITICAL method

> ⚡ **Prefer §3.3 runtime dump.** The proxy-DLL dump gives perfect engine-decrypted
> plaintext with zero scheme hunting and no near-miss risk. Only use the GARbro/CxDec
> route below if you can't run the game. Kept because it explains the near-miss trap.

KiriKiri commercial games encrypt XP3 content with **CxDec** (per-file, hash-derived key).
GARbro ships ~360 reverse-engineered schemes. The scheme name is per-developer, NOT the
game title (e.g. Taimanin/Lilith decrypts with the **"Himesho!"/"Papikon"/"Sis x Miko"**
family; My Ditzy with **"Ama Koi Syrups"**).

### How to find the scheme (fully headless/CLI)
1. Dump one sample (largest `.ks`) per scheme: `garbro_dump_samples.ps1 <xp3> <out_dir>`
   (uses GARbro reflection + a `ParametersRequest` handler to inject each scheme).
2. Score by **TEXT COHERENCE**: `coherence_scan_samples.py <out_dir>`.
3. The correct scheme has **replacement-char ratio ≈ 0** AND many common Japanese
   **particles** (の は を に た て が…) AND real hiragana density. Extract with it via
   `garbro_extract_paths.ps1 <xp3> <out> "<scheme>"` and confirm dense hiragana.

### ⚠️ THE key lesson (cost me many hours)
- **NEVER validate a scheme by raw hiragana/CJK byte-pair counts.** A partial/wrong
  CxDec decrypt produces many bytes in the SJIS hiragana lead-byte range (0x82 xx) and
  CJK code points BY CHANCE → huge false-positive scores. I twice "found" a scheme
  (code-point count, then hiragana-byte count) that was pure mojibake.
- **Only text COHERENCE is trustworthy**: decode → replacement-char ratio near 0 +
  presence of real particle words + no fragmented garbage in a skeleton view.
- Also **sample the LARGEST `.ks`** (the main scenario) — sampling a plugin/gallery/config
  file gives false negatives (little dialogue even when correctly decrypted).

### LPK packer
- Taimanin ships `LPK-10007.exe` + `.cf` (LPK = a KiriKiri exe packer). The archive
  scheme still came from GARbro's catalog; LPK mainly protects the exe. If a game's
  scheme is NOT in any catalog, the key must come from the running game (§3) or static
  exe RE (deep, uncertain).

---

## 3. KiriKiri proxy DLL — the DEFINITIVE inject + extract method  ✅ SOLVED

This one technique replaces ALL the CxDec-scheme / custom-patch-format struggle below.
Build a proxy DLL from **KirikiriTools** (arcusmaximus) source and it BOTH (a) serves
translated loose files over the game's archives AND (b) dumps engine-decrypted plaintext.
Works on encrypted, LPK-packed, and custom-patch-format games alike — because the game's
OWN engine does the decryption; we just hook `tTVPXP3Archive::CreateStreamByIndex`.

### 3.1 Build the proxy (VS 2022 BuildTools, MSVC v143, x86/Release)
- Source: `KirikiriTools/KirikiriUnencryptedArchive` (uses Detours; the copy under
  `D:\cosas\mis proyectos\Programacion\kirikiri\` has Detours populated).
- The DLL must be named after a DLL **the game's exe actually IMPORTS** (check with
  `dumpbin /imports <exe>`). KirikiriTools ships a prebuilt **`version.dll`** — works only
  if the exe imports version.dll (Ochiru's does). Many exes (My Ditzy `yurufuwamama.exe`,
  Taimanin `LPK-10007.exe`, Motto `hbunny.exe`) import **`winmm.dll`** instead → build a
  winmm variant:
  - `exports.def` → `LIBRARY winmm`; `<TargetName>winmm`.
  - A winmm proxy MUST export **ALL ~192 winmm functions** (other system DLLs e.g.
    MSVFW32 import `mciSendStringW` etc. → "entry point not found" crash if any is
    missing). Easiest: `#pragma comment(linker, "/EXPORT:name=winmm_orig.name")` for every
    export (get the list via `dumpbin /exports C:\Windows\SysWOW64\winmm.dll`), and drop a
    copy of the real SysWOW64 winmm.dll into the game folder as **`winmm_orig.dll`**.
  - `Kirikiri.cpp` `IsKirikiriExe()` calls version-resource APIs — call them directly
    (`#pragma comment(lib,"version.lib")`) instead of via the (now winmm) Proxy forwarders.
  - Remove `register` from `Kirikiri/tTJSHashTable.h` (illegal in C++17, which v143 uses).
- The built winmm.dll + winmm_orig.dll are reusable across ALL winmm-importing KiriKiri
  games (kept in session scratchpad `krkbuild/…/Release/`).

### 3.2 Inject a translation — `unencrypted/` folder override
- Drop the proxy DLL (+ winmm_orig.dll) in the game folder.
- Put translated loose files at `<game>/unencrypted/<item-path>` (e.g.
  `unencrypted/scenario/00_000.ks`, or `unencrypted/newgame.ks` — the path MUST match the
  archive item name the game requests, lowercase, as read from the archive).
- Modified `CustomCreateStreamByIndex` checks `unencrypted/<item>` first and serves it via
  `TVPCreateIStream` + `TVPCreateBinaryStreamAdapter`. This routes through the ALWAYS-
  installed CreateStreamByIndex hook, so it works even when the storage-media hook never
  fires (it didn't on My Ditzy). No need to load a patch.xp3 / match the custom format.
- **ONLY override pure DIALOGUE files.** Overriding system/`iscript`/`*macro` files
  (`define.ks`, `laynumber_init.ks`, `extraButton.ks`, config/plugin_ks) → KAG syntax or
  `*label not found` crashes. Keep those original.

### 3.3 Extract clean plaintext — `dump.txt` runtime dump (beats CxDec scheme hunting)
- Create an empty `<game>/dump.txt`, launch the game once, close it.
- On first access to each archive the hook dumps ALL `.ks` entries (engine-decrypted) to
  `<game>/dump/<archive-basename>/<item>` — force-dump-all, so one launch grabs everything
  (no playthrough needed). Per-archive folders let you pick English (`patch2.xp3`) vs
  Japanese (`data.xp3`). **This gives PERFECT plaintext with no scheme guessing** — the
  fix for near-miss CxDec (§2, §6b) and for games with NO catalog scheme.
- KiriKiriZ (Motto): scripts are `.scn` (PSB), not `.ks` — dump would need the `.ks` filter
  widened to `.scn`, then FreeMote to decompile the dumped PSB before translating.
- Remember to DELETE `dump.txt` before shipping (else it re-dumps every launch).

### Legacy runtime tools (superseded by 3.1–3.3, kept for reference)
- **KrkrExtract** (xmoezzz): same idea (runtime dump) but a separate GUI tool, drag-drop,
  not scriptable. Our proxy does it headlessly.
- **Text-hookers** (ITH/ITHVNR, VNR): capture on-screen text for a human READING; useless
  for patching (no stable file/line IDs, read-only, needs full playthrough).

---

## 4. Tools reference
| Tool | Use | URL |
|---|---|---|
| GARbro | Extract many archive formats; CxDec scheme DB (`GameData/Formats.dat`) | github.com/morkt/GARbro |
| krkr-xp3 | Pure-Python XP3 **read + repack** (unencrypted); `xp3.py -m e/-m r` | github.com/awaken1ng/krkr-xp3 |
| arc_unpacker | CLI multi-format extractor; explicit `--dec=kirikiri/xp3 --plugin=` (small plugin list) | github.com/vn-tools/arc_unpacker |
| VNTextPatch | Extract/inject VN text (KiriKiri ks + scn, YU-RIS, many engines) | github.com/arcusmaximus/VNTranslationTools ; net8 fork: rafael-vasconcellos/VNTextPatch-net8 |
| FreeMote | Decompile/recompile PSB `.scn/.psb/.mdf` | github.com/UlyssesWu/FreeMote |
| KrKrZSceneManager | `.scn` string editor (KiriKiriZ) | github.com/csh1668/KrKrZSceneManager |
| KiriKiriSCNJSONParser | Dump text from decompiled SCN JSON → txt | github.com/HoodedTissue/KiriKiriSCNJSONParser |
| KirikiriTools (Xp3Pack) | Make unencrypted patch.xp3 (loader reads it) | github.com/arcusmaximus/KirikiriTools |
| KrkrExtract | Runtime dump of a running KiriKiri game | github.com/xmoezzz/KrkrExtract |
| YPF.exe | Pack/unpack YU-RIS YPF | (ships with the YU-RIS game / VNTextPatch) |
| Guides | Dreamsavior "Patching KAG Games"; Fuwanovel Kirikiri patch-making | dreamsavior.net ; forums.fuwanovel.moe |

---

## 5. Status of the 5 games in D:\juegos\VN (as of this session)
| Game | Engine | Decrypt / Extract | Translate | Patch (inject) |
|---|---|---|---|---|
| Injuu Kangoku RE | YU-RIS | plaintext | ✅ JA→EN→ES | ✅ IN GAME — `ysbin.ypf` native tool |
| Ochiru Hitozuma | KiriKiri (.ks) | plaintext | ✅ EN→ES | ✅ IN GAME — `version.dll` proxy + zero-hash patch |
| My Ditzy Mom | KiriKiri (.ks) | ✅ runtime dump | ✅ EN→ES | ✅ IN GAME — **winmm.dll proxy + `unencrypted/`** |
| Taimanin Asagi PB | KiriKiri (.ks), LPK | ✅ runtime dump (patch2 EN) | ✅ EN→ES 31,914 lines | ✅ IN GAME — winmm proxy + `unencrypted/` (cp932, no accents) |
| Motto Haramase | KiriKiriZ (.scn PSB, **Hxv4** scn.xp3) | 🔄 runtime-dumpable per scene; static blocked by CxDec | cycle proven | pending — see below |

**4/5 in Spanish and confirmed in-game.** Motto: the inject+translate CYCLE is proven
(dump a `.scn` via the psb-media `extract-unencrypted.txt` hook → `VNTextPatch extractlocal`
gives clean JA → translate → `insertlocal` → serve via `unencrypted/`; VNTextPatch +
FreeMote.Psb.dll ship inside Injuu's `VNTranslationTools/`). The wall is getting ALL `.scn`
at once: `scn.xp3` uses the **Hxv4** custom XP3 (names stripped, bodies CxDec-encrypted) —
fully reversed in **`HXV4_FORMAT_RE.md`**, but the per-file CxDec key derivation is uncracked,
so static full extraction needs exe RE or many dumped pairs. Practical path today: dump the
scenes you play. `.scn` are uncompressed `PSB\0`; `adlr` = adler32(plaintext) = a decryption
oracle.

## 6. Working files
- Reusable scripts: `docs/vn-tools/` (this repo).
- Per-game DBs, extracted trees, patch trees, translation queues: `D:\juegos\parches\locust-tests\`.
- krkr-xp3 clone + arc_unpacker: under that folder / session scratchpad.
- GARbro install: `C:\Program Files (x86)\GARbro\`.

## 6b. Reinjection encoding — HARD-WON lessons (read before patching KiriKiri)
- **Match the ORIGINAL file's encoding EXACTLY.** KiriKiri reads patch `.ks` as
  ANSI via the system codepage OR UTF-16 (BOM). Writing UTF-16 where the engine
  expects cp932 corrupts label parsing → "シナリオファイル name.ks 内にラベル *filetop
  が見つかりません" / "ANSI文字列をUNICODE文字列に変換できません" errors.
- **cp932 (Shift-JIS) can't hold á/é/í/ó/ú/ñ/¿¡.** For cp932 patches you MUST
  transliterate accents to ASCII (reinject_bytes.py default). Only where the
  original file is UTF-16 (BOM ff fe) can you keep accents (`--keep-accents`).
- **Use BYTE-LEVEL reinjection (reinject_bytes.py), not decode+re-encode.**
  cp932 decode→str→encode is NOT byte-identical (ambiguous mappings), which
  corrupts label/tag lines and breaks `*label` lookups. reinject_bytes keeps
  every non-translated line's original bytes and only rewrites dialogue lines.
- **English localized releases:** most of D:\juegos\VN shipped as ENGLISH
  (Ochiru/Taimanin/My Ditzy). Translate EN→ES from the file that ACTUALLY LOADS
  (Ochiru=patch.xp3, Taimanin=patch2.xp3, My Ditzy=data.xp3) and OVERWRITE that
  file (back it up). Adding a new patchN.xp3 does NOT load (engines load a fixed
  set). The hiragana-based extractor misses English — use kag_extract_en.py.
- **Near-miss CxDec** (e.g. Taimanin patch2.xp3): a scheme can decrypt the first
  ~128KB cleanly (fooling sample-based validation) yet leave sparse garbage bytes
  later in the file → strict decode fails, re-encode produces invalid bytes,
  engine errors. No catalog scheme is perfect; only the game's own key is →
  KrkrExtract runtime. Validate schemes by STRICT full-file decode, not just a sample.
- **Engine encoding by game:** Ochiru patch.xp3 = plain cp932; My Ditzy scenario
  = UTF-16 (accents OK!); Taimanin patch2 = cp932 but near-miss-encrypted;
  Injuu (YU-RIS ysbin) = cp932 (VNTextPatch), accents impossible.

## 7. Tooling gotchas (environment)
- `python - <<'PY' … PY < /dev/null` opens the REPL (redirect beats heredoc) and hangs.
  **Always write .py to a file and run `python file.py args < /dev/null`.**
- `VNTextPatch.exe` / `YPF.exe` / game exes fail via Git-bash ("Permission denied") but
  **run fine via PowerShell / pwsh**.
- `fd` can miss files under paths with spaces; fall back to `ls`/`find`.
- GARbro's own console/extract wrapper here is unreliable for plain/patch XP3s — validate
  a written XP3 with krkr-xp3's self round-trip, not GARbro.
