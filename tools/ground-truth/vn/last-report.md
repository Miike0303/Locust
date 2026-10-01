# VN ground-truth audit — HEAD b0a17d2fd2fa91c069cd6b536185ddc5a8b193a5

## 1. Data integrity first

| Probe | Requested | Result | Evidence |
|---|---:|---|---|
| xp3-real | 3 | **172/172 pre-existing patch.xp3 members deleted**; patch.xp3 172 → 1 member. patch2.xp3 54 → 54, unchanged. Locust reports 3 writes; 0/3 requested translations re-extract. Runtime was not launched. | [xp3-real-injection.json](xp3-real-injection.json); [xp3-real-inject.log](xp3-real-inject.log) |
| ypf-real | 3 | Direct aborted (exit 1): untracked `.locust-stage-*/previous`. 572/572 installed members byte-identical; 0 writes installed; 3 requested rows remain original. Private staging retained. Safe installation failure, not a passing round-trip. | [ypf-real-injection.json](ypf-real-injection.json); [ypf-real-inject.log](ypf-real-inject.log) |
| kag-prefix | 6 | exit 0; admitted rows failing exact re-extract 0/6; untouched row locator/source changes 0; extracted TJS body accepted for translation (see rows #27/#29/#30); no unrelated bytes changed | [kag-prefix-injection.json](kag-prefix-injection.json); [kag-prefix-inject.log](kag-prefix-inject.log) |
| kag-mixed-newlines | 3 | exit 0; admitted rows failing exact re-extract 3/3; untouched row locator/source changes 15; **5,381 bare CR → LF**; LF count 18 → 5,398; extracted rows 18 → 2,022. Mixed delimiter normalization also changes how continuation tags are tokenized. | [kag-mixed-newlines-injection.json](kag-mixed-newlines-injection.json); [kag-mixed-newlines-inject.log](kag-mixed-newlines-inject.log) |
| kag-controls | 3 | exit 1; admitted rows failing exact re-extract 0/3; untouched row locator/source changes 0; **3/3 unsafe control-removal requests safely rejected, game files unchanged** | [kag-controls-injection.json](kag-controls-injection.json); [kag-controls-inject.log](kag-controls-inject.log) |
| yuris-prefix | 9 | exit 0; admitted rows failing exact re-extract 0/9; untouched row locator/source changes 0; 0 untouched attribute payload/instruction/line-number/tail mutations; includes a FONT.NAME lookup translated to AUDIT MS Gothic (technical-data false positive) | [yuris-prefix-injection.json](yuris-prefix-injection.json); [yuris-prefix-inject.log](yuris-prefix-inject.log) |
| tyrano-prefix | 9 | exit 0; admitted rows failing exact re-extract 0/9; untouched row locator/source changes 0; includes JS/control-only rows accepted for translation; no unrelated bytes changed | [tyrano-prefix-injection.json](tyrano-prefix-injection.json); [tyrano-prefix-inject.log](tyrano-prefix-inject.log) |
| tyrano-controls | 3 | exit 1; admitted rows failing exact re-extract 0/3; untouched row locator/source changes 0; **3/3 unsafe control-removal requests safely rejected, game files unchanged** | [tyrano-controls-injection.json](tyrano-controls-injection.json); [tyrano-controls-inject.log](tyrano-controls-inject.log) |
| nscripter-prefix | 3 | exit 0; admitted rows failing exact re-extract 0/3; untouched row locator/source changes 0 | [nscripter-prefix-injection.json](nscripter-prefix-injection.json); [nscripter-prefix-inject.log](nscripter-prefix-inject.log) |

Responsible XP3 write: `KirikiriPlugin::inject` `C:/Projects/Locust/crates/formats/src/kirikiri.rs:1145` / :1146 overwrites fixed `patch.xp3`; selection ranks `patch2` higher at `C:/Projects/Locust/crates/formats/src/kirikiri.rs:767`. Newline loss: `normalize_newlines` `C:/Projects/Locust/crates/formats/src/kirikiri.rs:486`, called by `encode_ks_bytes` :432 and `apply_translations` :668.

YPF abort crosses plugin/shared infrastructure: `YurisPlugin::inject_under_lock` `C:/Projects/Locust/crates/formats/src/yuris.rs:1322` → `archive_replace::replace_files` `C:/Projects/Locust/crates/formats/src/archive_replace.rs:81`; retained `previous` stage :139 rejected by `C:/Projects/Locust/crates/core/src/injection_transaction.rs:859`. No shared-core fix is assigned in these plugin-only briefs.

## 2. Inventory and rejected candidates

| Local VN folder | Engine / evidence | Audited scope |
|---|---|---|
| Certified Mother Fucker | RenPy; `D:\juegos\VN\Certified Mother Fucker/renpy/` | out of these four plugins |
| Injuu Kangoku RE | YU-RIS; `D:\juegos\VN\Injuu Kangoku RE/res/ysc.ybn` YSCM and `res/yst00182.ybn` YSTB | 571 loose .ybn (YSTB oracle + YSCF title); real ysbin.ypf injection separate |
| Motto_Haramase_Honoo_no_Oppai_Isekai_Oppai_Bunny_Gakuen | KiriKiri/KAG; XP3 magic/index and shipped .ks; `D:\juegos\VN\Motto_Haramase_Honoo_no_Oppai_Isekai_Oppai_Bunny_Gakuen/data.xp3` | 6 loose initialization .ks, only 1 literal; **not whole-game recall**; hashed/extensionless archive names; gameplay payloads unexamined |
| My Ditzy Mom Got Picked Up by a Playboy at the Beach | KiriKiri/KAG; XP3 magic/index and shipped .ks; `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach/data.xp3` | 32 loose scenario scripts; 101 data.xp3 .ks members not included |
| Nozokibeya RJ153210 | GsPack/GsWin (DataPack5); `D:\juegos\VN\Nozokibeya RJ153210/System.dat` DataPack5; independent `specs/ArcGsPack.cs:61` | out of these four plugins |
| Ochiru Hitozuma | KiriKiri/KAG; XP3 magic/index and shipped .ks; `D:\juegos\VN\Ochiru Hitozuma/data.xp3` | 49 readable .ks from patch2.xp3; independent XP3 transport; archive injection also checks patch.xp3 |
| Taimanin Asagi Premium Box | KiriKiri/KAG; XP3 magic/index and shipped .ks; `D:\juegos\VN\Taimanin Asagi Premium Box/data.xp3` | 9 loose .ks, with mixed/bare-CR files; archive .ks not included (data:130, patch2:10) |
| UBAI Again EN V1.01 | Liar-soft XFL; `D:\juegos\VN\UBAI Again EN V1.01/scr.xfl` LB 01 00; independent `specs/ArcXFL.cs:46` | out of these four plugins |

| Other location / candidate | Disposition and evidence |
|---|---|
| `D:/juegos/parches/locust-tests/` | Rejected as independent truth: Locust-named experiment trees (`mditzy_*`, `taimanin_*`, `ochiru_*`, `_rt*`); inventoried only. |
| `D:/juegos/parches/Erospanish-ParcheLustEpidemic/`, `Traducción Español The Genesis Order/` and sibling RARs | Loose folders empty in inventory; title suggests unrelated game patches, engine unverified; no .ks/.ybn/NS container. RARs not treated as a VN oracle. |
| `D:/juegos/otro/fall.out/`, `The_Nun_v0.1.2_base/` | Engine unverified from this inventory; no target script/container marker. Excluded. |
| `D:/juegos/otro/marniegame 041.pck` and exe/ZIP copies | Godot PCK candidate, outside scope. |
| Existing KiriKiri `patch.xp3`, `patch2.xp3` and `.bak`/`.orig` | Used actual bytes for archive integrity; **not assumed to be translator-authored original/translation pairs**. Previous edits/provenance unknown. |
| Injuu `output/` vs `output.ja.bak/` | 412 paired files, same array lengths, 37,616 nonempty name/message fields, 25,448 changed. `.ja.bak` contains **English**, not Japanese. Provenance clues: `VNTranslationTools/run_extract.bat:2` names Dazed workspace; `gameupdate/patch-config.txt:1` names dazed-translations. Separate lexical cross-check, not asserted human translation or primary opcode oracle. See `translation-pairs.json`. |
| Python parser packages | `py -3.13` import checks: no krkr, xp3, yuris, pylzss, construct, chardet, asar, zstandard; charset_normalizer available but not a format oracle. No installs. |
| VNTextPatch executable | Copied third-party exe/DLLs into `vntp/`; process launch returned application loader 0xc0000142. Not claimed executed. Public parser source + independent readers used instead. |
| Public GBK NS example `corpus/NScripterPublic/0.txt` | Rejected: Chinese GBK demo from wcwac/em-onscripter; strict CP932 decode fails byte 177. No encoding-loss conversion used. See `nscripter-public-source.json`. |
| Tyrano / NS installed games | None identified in listed local trees. **Public third-party samples are reported separately**; see manifests below. |

## 3. Ground truth, definitions, denominators

| Oracle | Why independent / trustworthy | Scope and limits |
|---|---|---|
| KAG | Game scripts produced before audit, parsed by scratch `readers.py` from KiriKiri runtime grammar (`specs/KAGParser.cpp:1134` iscript, :1507 text, :1514 escaped bracket). NAME_W macro renders `%n` in actual `Taimanin.../unencrypted/name.ks:799`. | Static literal spans and selected known display attributes; no execution/reachability claim. My `[name text]` role inferred from repeated speaker/dialogue use (`00_000.ks:12`, :37, :45), not from a recovered custom macro implementation. Conditional macros and other display commands are not exhaustively evaluated. |
| YU-RIS | Actual `res/ysc.ybn` opcode/argument table + independent YSTB binary reader. Public `specs/YurisScenarioScript.cs:226` WORD and :236 ES.CHAR.NAME/ES.SEL.SET; :451 EF F0 newline; `YurisConfigScript.cs:12` caption. CGACT SETSTR with TEXT=1 recognized using descriptor parameter ids from `ysc.ybn`. | Physical displayed attribute occurrences, including punctuation literals; assignments and unproven indirect display roles remain unclassified. No Locust tests/database define the oracle. |
| Tyrano | 8 engine-author sample .ks files, URLs and SHA256 in `tyrano-public-sources.json`; own parser `specs/kag.parser.js:180` separates script bodies; glink handler `specs/kag.tag.js:7168`, ruby :6290, chara name `specs/kag.tag_ext.js:2056`. | Public sample, not local commercial-game recall. 5 ruby readings are inside opaque rich rows but not separately addressable as text attributes; strict typed-slot metric below counts them missing. |
| NScripter | Japanese demo `onscripter_jp_test/0.txt` distributed by external maintainer; SHA256/URL in `nscripter-jp-source.json`. Runtime `specs/ScriptHandler.cpp:202` text routing, `ONScripter_command.cpp:3447` caption, `ScriptParser_command.cpp:385` rmenu labels. | One public SJIS demo; no encrypted-container or whole-library inference. Raw script preserved; only relevant bytes extracted from ZIP. |

- **Recall** = oracle literal slots covered / oracle literal slots. Each slot is (file, physical span/attribute), duplicates count. Message spans may be carried in a rich whole-line row; tag attributes must be separately addressable source values. This is **strict typed-slot recall**, not raw substring discovery. No whitespace-only slots. Visible punctuation is counted and reported separately.
- **Precision bounds** = confirmed rows containing player text / all extracted rows, through (all rows − confirmed technical rows) / all rows. Unclassified rows remain in the denominator; KAG/Tyrano rich messages carrying controls are accepted as text-bearing rows. This is row precision, not token purity.
- CLI databases are measured output only. Oracle scripts never import Locust code/tests or use its database as truth. IDs only map output to independent physical slots; Yuris mapping uses text/order and has 0 unmapped baseline rows.
- KAG logical locations count CR/CRLF/LF; CLI IDs count LF only. Both are retained in evidence; physical script bytes remain the source of truth. Public URLs are backed by cached files and SHA256 (`specs/`, source manifests, especially `final-spec-sources.json`).

| Scope | Files | Oracle | Hit | Miss | Recall | Rows | Confirmed TP | FP | Unclassified | Precision lower–upper |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Motto_Haramase_Honoo_no_Oppai_Isekai_Oppai_Bunny_Gakuen | 6 | 1 | 1 | 0 | 100.00% | 1 | 1 | 0 | 0 | 100.00%–100.00% |
| My | 32 | 4,454 | 3,501 | 953 | 78.60% | 3,504 | 3,501 | 3 | 0 | 99.91%–99.91% |
| Ochiru | 49 | 33,978 | 33,771 | 207 | 99.39% | 33,767 | 33,767 | 0 | 0 | 100.00%–100.00% |
| Taimanin | 9 | 57,861 | 27,222 | 30,639 | 47.05% | 25,252 | 25,238 | 14 | 0 | 99.94%–99.94% |
| Injuu | 571 | 27,064 | 20,271 | 6,793 | 74.90% | 22,506 | 20,271 | 10 | 2,225 | 90.07%–99.96% |
| TyranoOfficial | 8 | 168 | 153 | 15 | 91.07% | 388 | 147 | 241 | 0 | 37.89%–37.89% |
| NScripterJP | 1 | 226 | 222 | 4 | 98.23% | 222 | 222 | 0 | 0 | 100.00%–100.00% |

Independent shipped-JSON lexical cross-check: **21,302/28,126 = 75.74%** unique (file, field, normalized value) found as a substring in matching-file Locust sources. Not occurrence recall and not precision. Full pair paths/values: `translation-pairs.json`.

## 4. Miss / false-positive classes — complete real examples

| Class | Count | Source / rationale |
|---|---:|---|
| kirikiri_miss_attribute_name_text | 953 | `C:/Projects/Locust/crates/formats/src/kirikiri.rs:531`; `C:/Projects/Locust/crates/formats/src/kirikiri.rs:655` |
| kirikiri_fp_script_body | 17 | `C:/Projects/Locust/crates/formats/src/kirikiri.rs:506`; `C:/Projects/Locust/crates/formats/src/kirikiri.rs:653`; engine `specs/KAGParser.cpp:1134` |
| kirikiri_miss_punctuation_message | 207 | `C:/Projects/Locust/crates/formats/src/kirikiri.rs:537`; `C:/Projects/Locust/crates/formats/src/kirikiri.rs:587` (deliberate filler filter; visible literal coverage, not a required prose translation) |
| kirikiri_miss_attribute_name_w_n | 10,392 | `C:/Projects/Locust/crates/formats/src/kirikiri.rs:531`; game `D:/juegos/VN/Taimanin Asagi Premium Box/unencrypted/name.ks:783` / `:799` (macro renders %n) |
| kirikiri_miss_bare_cr_message | 20,247 | `C:/Projects/Locust/crates/formats/src/kirikiri.rs:653`; `C:/Projects/Locust/crates/formats/src/kirikiri.rs:486` |
| yuris_miss_display_literal | 178 | `C:/Projects/Locust/crates/formats/src/yuris.rs:506`; `C:/Projects/Locust/crates/formats/src/yuris.rs:769`; caption `specs/YurisConfigScript.cs:12` |
| yuris_miss_word_control | 6,615 | `C:/Projects/Locust/crates/formats/src/yuris.rs:473`; `C:/Projects/Locust/crates/formats/src/yuris.rs:515`; independent `specs/YurisScenarioScript.cs:451` |
| yuris_fp_technical_attribute | 10 | `C:/Projects/Locust/crates/formats/src/yuris.rs:794`; `C:/Projects/Locust/crates/formats/src/yuris.rs:504`; argument definitions `D:/juegos/VN/Injuu Kangoku RE/res/ysc.ybn` decoded in `yuris-commands.json` |
| tyrano_miss_visible_attribute | 15 | `C:/Projects/Locust/crates/formats/src/tyrano.rs:367`; `C:/Projects/Locust/crates/formats/src/tyrano.rs:433`; engine `specs/kag.tag.js:7168`, `specs/kag.tag_ext.js:2056`, `specs/kag.tag.js:6290` |
| tyrano_fp_script_body | 156 | `C:/Projects/Locust/crates/formats/src/tyrano.rs:380`; engine `specs/kag.parser.js:180` / `:484` |
| tyrano_fp_quoted_bracket_tag | 85 | `C:/Projects/Locust/crates/formats/src/tyrano.rs:345`; engine quote parser `specs/kag.parser.js:169` and `:309` |
| nscripter_miss_display_attribute | 4 | `C:/Projects/Locust/crates/formats/src/nscripter.rs:308`; `C:/Projects/Locust/crates/formats/src/nscripter.rs:394`; engine `specs/ONScripter_command.cpp:3447`, `specs/ScriptParser_command.cpp:385` |

### kirikiri_miss_attribute_name_text — 953

1. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\00_000.ks` — line 12

```text
Vendor
```

Complete physical line:
```text
[name text="Vendor"]
```

2. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\00_000.ks` — line 37

```text
Mitsuki
```

Complete physical line:
```text
[name text="Mitsuki"]
```

3. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\00_000.ks` — line 45

```text
Vendor
```

Complete physical line:
```text
[name text="Vendor"]
```


### kirikiri_fp_script_body — 17

1. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\_first.ks` — logical line 27 CLI line 27

```text
if( tf.debugsys == null )
```

2. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\_first.ks` — logical line 29 CLI line 29

```text
	System.inform( 'debug.ksかrelease.ksが読み込まれていません' );
```

3. `D:\juegos\VN\My Ditzy Mom Got Picked Up by a Playboy at the Beach\unencrypted\scenario\_first.ks` — logical line 30 CLI line 30

```text
	System.exit();
```


### kirikiri_miss_punctuation_message — 207

1. `D:\juegos\VN\Ochiru Hitozuma\patch2.xp3/S000a.ks` — line 12

```text
..............................................................................
```

Complete physical line:
```text
　..............................................................................
```

2. `D:\juegos\VN\Ochiru Hitozuma\patch2.xp3/S000a.ks` — line 13

```text
.................................................................................
```

3. `D:\juegos\VN\Ochiru Hitozuma\patch2.xp3/S000a.ks` — line 14

```text
.................................................................................
```


### kirikiri_miss_attribute_name_w_n — 10,392

1. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 43

```text
Asagi
```

Complete physical line:
```text
[NAME_W n="Asagi"]\
```

2. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 62

```text
Asagi
```

Complete physical line:
```text
[NAME_W n="Asagi"]\
```

3. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 78

```text
Asagi
```

Complete physical line:
```text
[NAME_W n="Asagi"]\
```


### kirikiri_miss_bare_cr_message — 20,247

1. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 44

```text
「Eh, e-espera...... nuuuuuh!!」
```

Complete physical line:
```text
「Eh, e-espera...... nuuuuuh!!」[T_NEXT]\
```

2. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 52

```text
「Miiira, voy a entrar, voy a entrar. Ohhhhhhhhhhhhhhhhhah, ya entre!」
```

Complete physical line:
```text
「Miiira, voy a entrar, voy a entrar. Ohhhhhhhhhhhhhhhhhah, ya entre!」[T_NEXT]\
```

3. `D:\juegos\VN\Taimanin Asagi Premium Box\unencrypted\movie_seen01.ks` — line 57

```text
Sin preliminares, Pig me penetro de golpe hasta el fondo.
```

Complete physical line:
```text
 Sin preliminares, Pig me penetro de golpe hasta el fondo.[T_NEXT]\
```


### yuris_miss_display_literal — 178

1. `D:\juegos\VN\Injuu Kangoku RE\res\yscfg.ybn` — caption length at 0x4c, bytes 0x4e..

```text
Prision de la Bestia: RE
```

2. `D:\juegos\VN\Injuu Kangoku RE\res\yst00160.ybn` — instruction 10 opcode GOSUB argument 1 attr 45

```text
Liz
```

3. `D:\juegos\VN\Injuu Kangoku RE\res\yst00160.ybn` — instruction 10 opcode GOSUB argument 2 attr 46

```text
Liz
```


### yuris_miss_word_control — 6,615

1. `D:\juegos\VN\Injuu Kangoku RE\res\yst00182.ybn` — instruction 23 opcode WORD argument 0 attr 81

```text
Researcher「 Ngh, aahh! Mmhh... oogh!! ...Hff... gugh! Ah...! No... hah...
ah, augh... ngh... fuuuh... no... aooh!」
```

2. `D:\juegos\VN\Injuu Kangoku RE\res\yst00182.ybn` — instruction 30 opcode WORD argument 0 attr 100

```text
Es demasiado burdo para llamarlo un registro apropiado, pero debo
anotar todo sobre esta investigacion.
```

3. `D:\juegos\VN\Injuu Kangoku RE\res\yst00182.ybn` — instruction 33 opcode WORD argument 0 attr 109

```text
Hasta ahora, me acercaba a mi propia investigacion con sinceridad y
sentia una curiosidad honesta, orgullo y pasion por descubrir lo
desconocido.
```


### yuris_fp_technical_attribute — 10

1. `D:\juegos\VN\Injuu Kangoku RE\res\yst00023.ybn` — instruction 204 opcode FONT argument 0 attr 372

```text
MS Gothic
```

2. `D:\juegos\VN\Injuu Kangoku RE\res\yst00036.ybn` — instruction 276 opcode GOSUB argument 0 attr 512

```text
４５４５定義
```

3. `D:\juegos\VN\Injuu Kangoku RE\res\yst00041.ybn` — instruction 162 opcode FILEACT argument 1 attr 275

```text
IExplore
```


### tyrano_miss_visible_attribute — 15

1. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/scene1.ks` — line 33; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\scene1.ks`

```text
あかね
```

Complete physical line:
```text
[chara_new  name="akane" storage="chara/akane/normal.png" jname="あかね"  ]
```

2. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/scene1.ks` — line 42; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\scene1.ks`

```text
やまと
```

Complete physical line:
```text
[chara_new  name="yamato"  storage="chara/yamato/normal.png" jname="やまと" ]
```

3. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/scene1.ks` — line 67; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\scene1.ks`

```text
はい。興味あります
```

Complete physical line:
```text
[glink  color="blue"  storage="scene1.ks"  size="28"  x="360"  width="500"  y="150"  text="はい。興味あります"  target="*selectinterest"  ]
```


### tyrano_fp_script_body — 156

1. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/cg.ks` — logical line 18 CLI line 18; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\cg.ks`

```text
    tf.page = 0;
```

2. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/cg.ks` — logical line 19 CLI line 19; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\cg.ks`

```text
    tf.selected_cg_image = ""; //選択されたCGを一時的に保管
```

3. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/cg.ks` — logical line 32 CLI line 32; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\cg.ks`

```text
    tf.tmp_index = 0;
```


### tyrano_fp_quoted_bracket_tag — 85

1. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/config.ks` — line 104; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\config.ks`

```text
	[button name="bgmvol,bgmvol_10"  fix="true" target="*vol_bgm_change" graphic="&tf.btn_path_off" width="&tf.btn_w" height="&tf.btn_h" x="&tf.config_x[1]"  y="&tf.config_y_bgm" exp="tf.current_bgm_vol =  10; tf.config_num_bgm =  1"]
```

2. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/config.ks` — line 105; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\config.ks`

```text
	[button name="bgmvol,bgmvol_20"  fix="true" target="*vol_bgm_change" graphic="&tf.btn_path_off" width="&tf.btn_w" height="&tf.btn_h" x="&tf.config_x[2]"  y="&tf.config_y_bgm" exp="tf.current_bgm_vol =  20; tf.config_num_bgm =  2"]
```

3. `https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/data/scenario/config.ks` — line 106; cached `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\TyranoOfficial\data\scenario\config.ks`

```text
	[button name="bgmvol,bgmvol_30"  fix="true" target="*vol_bgm_change" graphic="&tf.btn_path_off" width="&tf.btn_w" height="&tf.btn_h" x="&tf.config_x[3]"  y="&tf.config_y_bgm" exp="tf.current_bgm_vol =  30; tf.config_num_bgm =  3"]
```


### nscripter_miss_display_attribute — 4

1. `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\NScripterJP\0.txt` — LF line 4

```text
みずいろ01
```

Complete physical line:
```text
caption "みずいろ01"
```

2. `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\NScripterJP\0.txt` — LF line 24

```text
選択肢まで進む
```

Complete physical line:
```text
rmenu "選択肢まで進む",skip,"ウィンドウを消す",windowerase,"最初に戻る",reset
```

3. `C:\Users\Mike\AppData\Local\Temp\locust-research-vn\corpus\NScripterJP\0.txt` — LF line 24

```text
ウィンドウを消す
```

Complete physical line:
```text
rmenu "選択肢まで進む",skip,"ウィンドウを消す",windowerase,"最初に戻る",reset
```


## 5. Ranked writer work — disjoint plugin ownership

| Rank | Size | Single file | Defect cluster / exact symbols | Baseline / minimum failing regression | Brief |
|---|---|---|---|---|---|
| 1 | L | **kirikiri.rs** | `KirikiriPlugin::inject` :1145; `normalize_newlines` :485; `extract_lines_from_text` :651; `is_non_text_line` :506 | XP3 deletes 172 untouched members and 0/3 round-trip; mixed CR 5,381 conversions; 20,247 bare-CR literal misses; 11,345 display-attribute misses; 17 TJS FP. Minimal executable fixtures: `regressions/kag-xp3-integrity` loses keep.bin and hides translated story; `regressions/kag-newline-integrity` adds BOM, changes unrelated CRs and drops final separator; `;comment\rHello.\rGoodbye.\r` extracts 0/2. | [writer-01-kirikiri.md](writer-01-kirikiri.md) |
| 2 | L | **yuris.rs** | `decode_attr_value` :461; `looks_player_visible` :504; `load_ystb` :764; `serialize_attr_value` :723 | 6,615 control-bearing WORD misses; 178 other visible literal misses incl title; 10 confirmed technical FP. Minimal WORD raw `Hello EF F0 world` extracts 0/1; read `regressions/yuris-word-control/story.ybn`. | [writer-02-yuris.md](writer-02-yuris.md) |
| 3 | M | **tyrano.rs** | `classify_lines` :380; `is_pure_tag_line` :331; `entries_from_ks_bytes` :433 | 156 JS + 85 pure tag FP; 15 strict attribute misses (8 choices, 2 jname, 5 ruby); precision 147/388. Minimal iscript+JS+glink emits JS but no choice; eval exp with nested bracket emitted as text. | [writer-03-tyrano.md](writer-03-tyrano.md) |
| 4 | M | **nscripter.rs** | `is_player_text_line` :307; `NScripterPlugin::extract` :360; `NScripterPlugin::inject` :408 | 4 caption/rmenu literal misses, 222/226 recall; minimal `rmenu "Skip",skip,"Hide",windowerase,"Restart",reset` yields 0/3 labels. | [writer-04-nscripter.md](writer-04-nscripter.md) |

No briefs edit unity.rs, unity_serialized.rs, shared core, archive_replace.rs, or archive reader files. Each writer owns one plugin file. Shared YPF staging failure is recorded for coordinator routing; it is not disguised as a successful archive injection.

## 6. Reproduction / verification

- Integrity regressions: `regression-results.json`, `kag-xp3-integrity-regression-inject.log`, `kag-newline-integrity-regression-inject.log`. Minimal synthetic XP3 uses public `specs/ArcXP3.cs:97/:111/:150/:216`; 1 untouched member deleted, 0/1 effective translation. Minimal mixed-newline fixture preserves an LF target but has two unrelated CR separators: old inject also adds an UTF8 BOM and drops the final separator. This supplements, and does not replace, real-game integrity evidence above.
- One command: `py -3.13 run_all.py`. Writer CLI: `py -3.13 run_all.py --cli C:/.../locust.exe`. `regressions.py --require-fixed` is a post-fix gate; baseline records eight expected failing checks without stopping the audit.
- Build: `cargo build --locked -p locust-cli` ran only in scratch `snapshot/`, produced `target/debug/locust.exe`; source from `git archive HEAD`. CARGO_TARGET_DIR/TEMP/TMP/LOCUST_DATA_DIR stay under this audit directory. No cache copied or written. No repo build/commit/edit.
- `original-hashes-before.json` / `original-hashes-check.json`: **3,310 sampled original files, 0 SHA256 changes**. Scope: all audited loose .ks/.ybn/.json/.tjs plus selected data/patch/patch2/scn XP3 and ysbin YPF(+bak), excluding huge media archives. This is a sample of game trees, not every game byte.
- `source-hashes.json`: audited eight plugin sources match pinned HEAD after LF normalization; checkout CRLF bytes differ from git-archive LF. `repo-status-after.txt` has only the two pre-existing Unity writer files modified. No other tracked changes.
- Independent hand checks: `hand-checks.md`; full machine evidence: `misses.json`, `false-positives.json`, `*-oracle.json`, `*-injection.json`, extraction/inject logs, `translation-pairs.json`, `regression-results.json`. Installed game runtimes were not launched.
- Archive denominator: XP3 original member loss is independently decoded; YPF failure preserves 572 members. Successful loose Yuris injection compares every untouched attribute payload and instructions/line-number/tail bytes, not just write counts. Failed control requests are distinguished from admitted round-trip failures.
- No real Windows sandbox command-runner helper failure occurred. PowerShell profile warnings on initial reads and VNTextPatch application-loader failure were not treated as sandbox-helper failures.
