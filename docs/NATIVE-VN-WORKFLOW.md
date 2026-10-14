# Translate a Japanese visual novel safely in the native app

Use Locust's desktop editor to translate **readable, supported assets** through Japanese → English → Spanish, then create a separate game copy. No Python or helper scripts are required for this workflow. These are separate, checked steps—not a one-click pipeline. A complete database does not prove a complete or playable game translation.

**Keep the original game and saved projects intact. Never choose “Direct” unless you deliberately intend to write into the selected game folder.** Its backup is not a reason to use it here. “Add” also writes inside that folder; this guide uses Replace copies instead.

## 1. Open first, then confirm the source

- Choose “Open Game Folder” / “Abrir carpeta del juego”, or “Open Game File” for a supported file. To resume saved work, use “Open project database” / “Abrir base del proyecto”. **Open the game/project before changing translation languages**: defaults can change with project context.
- Inspect actual source rows, not just the engine name or language default. Confirm that readable display text is Japanese. If it is already English, translate **English → Spanish** directly; an English KiriKiri sample is not evidence of Japanese → English support.
- Check the detected engine, stability label and extraction warnings. Empty, garbled, encrypted, unsupported or ambiguous results are a reason to stop—not to translate noise or force injection. Loose files and archives can disagree; extraction alone does not establish which version the game loads.

| Assets | Boundary |
| --- | --- |
| Ren'Py | Stable plugin; supported readable scripts/RPA. Compiled-only assets do not by themselves establish readable Japanese coverage. |
| KiriKiri/KAG | Experimental; supported loose `.ks` and unencrypted XP3/script encodings. No cxdec decryption. |
| YU-RIS | Experimental; supported YSTB/YPF layouts, not every archive scheme or runtime command. See the encoding stop below. |
| Other engines | Consult the [supported-engine table](../README.md#supported-game-engines). An unidentified DAT/XFL or protected/hashed script is not made supported by renaming it. |

For an older YU-RIS project without verified literal metadata, obtain a **fresh, verified extraction from a separate untouched game copy into a separate project**. Preserve the old saved database and translations; do not overwrite them, silently migrate metadata or refresh a pivot to acquire it. Compare coverage and provenance before using the new project.

## 2. Finish Japanese → English, then pivot

1. In “Translate” / “Traducir”, explicitly select Japanese as source and English as target, and choose your configured provider. Review errors and unfinished rows; a job ending is not proof that every row translated.
2. Review the English text and protected controls. Finish every extracted row before selecting “Pivot” / “Pivotar” → “Create project…” / “Crear proyecto…”. Save to a **new** database filename.
3. Native Pivot requires a nonempty project with a nonblank translation on every row, translated/reviewed/approved status, current source and preserved protected controls. It refuses incomplete work before creating the output database. The displayed status count is informational, not a substitute for this gate; correct rejected rows rather than dropping them to force completion.
4. Choose “Open this project” / “Abrir este proyecto”. This opens the pivot database **without re-extracting**. English becomes its semantic source; the physical Japanese injection baseline and locators remain preserved. The Japanese → English database is retained separately.
5. In the pivot project's Translate dialog, explicitly select **English → Spanish**. Do not reopen the game folder to start this stage: that is an extraction path, not the saved pivot. On a later session, open the pivot database instead.

## 3. Validate—and stop on incompatible encoding

Run “Validate” / “Validar” and inspect the issue list before copying. Resolve missing translations, stale sources, damaged controls/placeholders and applicable encoding or binary-slot issues. Validation is not linguistic, font or whole-engine certification.

**Current YU-RIS stop:** the bounded quoted-literal path uses Shift-JIS, not UTF-8. With exact extractor-owned literal provenance, Validate reports “Encoding incompatible” / “Codificación incompatible” for unrepresentable targets. Selected incompatible translations are refused by shared injection preflight **before backup or output-copy creation**. Saved semantic Spanish and pivot provenance remain unchanged.

This path does not silently remove accents, fit the text into an invented byte budget, or enable Unicode in the game. Complete accented Spanish remains blocked when the target cannot be represented. Do not strip accents or bypass the guard to call the result complete. Raw controls, captions and old/unmarked rows are **outside this specific literal/encoding guarantee**; a clean validation result there does not establish coverage. Literal bracket handling does not prove that an indirect script argument is displayed dialogue.

## 4. Create and inspect the translated copy

1. Select “Inject” / “Inyectar”. Choose “Replace — create a translated copy (original untouched)” / “Reemplazar — crear una copia traducida (original intacto)”. Replace-only engines default to this mode, but check the selection every time.
2. Select only the language actually translated in this database. Choose a separate “Output folder” / “Carpeta de salida”, outside the original game. Review the proposed destination, then select “Create translated copy” / “Crear copia traducida”. Do not choose “Direct” to work around a refusal.
3. Inspect the **actual returned translated-copy path**, not just the chosen parent folder or predicted name. Check strings written, files modified, skipped counts, skip reasons, warnings, failed languages and recording problems. “Injection complete” is not whole-game coverage; unexplained skips or unchanged Japanese prevent a complete-result claim.
4. Inspect and test only that copy. Confirm the intended files and text, controls, menus and runtime behavior; check font rendering and language quality separately. Do not proceed on a partial or refused result.

## 5. Pack only a verified complete result (optional)

Use “Patch” / “Parche” → “Pack” / “Empaquetar” only after verifying the copy and its recording. Point the game folder at the **actual translated-copy root** from Replace, not the original or output parent. Pack uses recorded written files; it does not repair skips or prove completeness. Direct injection is **not** required to package a Replace recording. Verify any resulting patch, installation and rollback on another disposable copy, never the original.

## Evidence and remaining limits

A bounded Japanese-bearing YU-RIS sample produced **22/22 English writes, zero skips**, exact re-extraction and pack/install/rollback integrity evidence. It included 3 definite display rows and 19 indirect arguments; it was not a whole-game or definite-dialogue inventory. The Spanish database retained all 22 semantic translations, but 17 were Shift-JIS-incompatible; current native validation and preflight block that output rather than presenting it as complete.

These results do **not** prove a native Tauri window, game runtime, font coverage or linguistic quality. Sampled English KiriKiri assets require the English → Spanish route, not a fabricated Japanese-source claim.

Future runtime integration is research only. The primary [VNTranslationTools README](https://github.com/arcusmaximus/VNTranslationTools/blob/main/README.md) describes Shift JIS extension through unused code points, `sjis_ext.bin` and a VNTextProxy runtime DLL—not a native UTF-8 payload switch. No such proxy was installed for this work or supplied by this workflow; integration would need separate design, runtime and font verification.
