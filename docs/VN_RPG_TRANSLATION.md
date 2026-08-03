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

### Locust patch apply (shared by all engines)

After inject has **recorded** what it wrote, package and apply without hand-unzipping:

```bash
# Prefer a pristine tree for original hashes (strict verify on apply)
locust patch "<injected_or_recorded_game>" -P project.locust.db -l es -o game-es-patch.zip --pristine "<pristine_game>"

# Apply onto a clean copy of the game (never the only original without backup)
locust apply "<clean_game_copy>" game-es-patch.zip
locust patch-status "<clean_game_copy>"
locust patch-rollback "<clean_game_copy>"   # restores .locust/backup
```

- Backups and receipts live in `<game>/.locust/` (hidden on Windows).
- Server: `POST /api/patch/{verify,apply,rollback,status}` (binds **127.0.0.1** by default).
- Desktop: Editor → **Patch** (Ctrl+Shift+P).
- **Unity**: translations must be **≤ source UTF-8 byte length** or inject skips them (no hard fail).
- **Wolf**: translations must be **≤ source Shift-JIS byte length** or inject skips them (no hard fail).
- **Unreal**: translations must be **≤ source UTF-16LE byte length** or inject skips them (no hard fail).
  The `mock` provider is length-safe for **both UTF-8 and UTF-16LE** slots (ASCII
  tags no longer blow Unreal's UTF-16 budget on short CJK); SJIS usually follows.
  Extract tags entries with `metadata.binary_slot` (`utf8` / `utf16le` / `sjis`);
  `locust validate <db>` reports `ExceedsBinarySlot` before inject.

**Phase-2 apply proven (copies only, mock or equal-length where needed):** RPG Maker MV, MZ, XP/VXA, Ren'Py, SugarCube, HTML generic (non-SugarCube fixture), Unity (BOXMAN), Unreal (Last Hope `_P.pak` subset — full 8GB base pak not copied), Wolf RPG (synthetic `Data/*.wolf` fixture — no commercial title on disk yet), VNTextPatch JSON (synthetic `yst*.json` fixture — re-run VNTextPatch to pack into the original engine).

`locust inject` / `inject --direct` preflight: warns if any translation exceeds a tagged binary inject slot (see `locust validate`).

- **Locust DB**: each string has `id`, `source`, `translation`, `status`, `file_path`.
  For VNTextPatch-format games the `id` is `<jsonname>.json#<index>#message`, which
  maps back to the source file + line for reinjection.
- **Translate**: `locust translate <db> -p grok-sub -s ja -t en --concurrency N --batch-size B --context "..."`.
  Only `status=Pending` rows are sent, so re-running resumes cleanly.
- **Pivot** (bridge language): `locust pivot <ja_db> -o <en2es_db>` copies each EN
  translation as the SOURCE of a new DB → translate `-s en -t es`. Then `-s en -t fr`, etc.
- **Neutral LatAm Spanish** is applied automatically by the provider prompt for `es*`.

### Translation gotchas (all engines)
- **Count mismatch** (`sent 20 got 19`): grok merges adjacent short lines and the
  count guard rejects the batch. The SAME batch fails every retry → permanent stall.
  **Fix: shrink batch size** on stall (20→12→6→3→2). Small batches don't merge.
  See `tonight_finish.sh` / `taimanin_translate.sh` `tr_until_done()`.
- **Cost/usage**: grok-sub is a subscription ($0/token). Runs are logged in the DB
  `translation_runs` table — `locust stats <db>` shows strings/tokens(in/out)/time.
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

### KiriKiri (KAG, `.ks` text scripts) — MEDIUM  ✅ done: Ochiru
- Scripts are `.ks` text (cp932/Shift-JIS, or UTF-16LE with FF FE BOM), stored in `data.xp3`.
- The game auto-loads `patch.xp3`, `patch2.xp3`, … as **overlays** over `data.xp3`
  (higher number wins), UNENCRYPTED. **A translation patch never re-encrypts anything.**
- **Pipeline (Ochiru worked end-to-end):**
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

### YU-RIS (`.ybn` in `.ypf`) — MEDIUM  ✅ done: Injuu
- DLLs `YSOHC/YSPNG/YSSNP/YSTCH/YSWBP/YSZLB`, archives `pac/*.ypf`, config `yscfg.dat`.
- **VNTextPatch** (arcusmaximus) extracts/injects YU-RIS text; **`YPF.exe`** packs/unpacks YPF.
- Injuu shipped a ready orchestrator `ysbin-patch.ps1` (`unpack` / `pack`):
  - unpack: `YPF.exe -e ysbin.ypf` → `VNTextPatch extractlocal ysbin output` (→ `output/*.json`)
  - pack:   `VNTextPatch insertlocal ysbin output res` → merge → `YPF.exe -c ysbin -v 500`
- To translate: write ES into (a copy of) `output/*.json`, then run `pack`. It backs up
  `ysbin.ypf` → `.bak`. **Run with pwsh 7** (`C:\Program Files\PowerShell\7\pwsh.exe`) —
  the script has a parse error under Windows PowerShell 5.1.
- Locust DB id `<name>.json#<i>#message` maps directly to `output/<name>.json[i]["message"]`
  (VNTextPatch local JSON). See `inject_injuu_json.py`.

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
