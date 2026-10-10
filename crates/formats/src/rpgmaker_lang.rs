//! Register an extra UI language on deployed RPG Maker MV/MZ multi-lang titles.
//!
//! Handles the common Waterbear / Iavra + VisuMZ OptionsCore pattern and
//! title-map Show Choices language pickers (as in Elf-Goblin United Front).
//!
//! Not a full VisuMZ editor — best-effort string surgery with backups.

use std::path::{Path, PathBuf};

use locust_core::database::{Database, RecordedBackup};
use locust_core::encoding::EncodingDetector;
use locust_core::error::{LocustError, Result};
use locust_core::injection_transaction::{
    add_recording_under_lock, ensure_no_pending_under_lock, write_files_and_record_under_lock,
};
use locust_core::patch::GameLock;
use serde::Serialize;

use crate::rpgmaker_mv::RpgMakerMvPlugin;

/// Report of what `register_language` changed on disk.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RegisterLanguageReport {
    pub plugins_js: bool,
    pub iavra_languages: bool,
    pub visumz_options: bool,
    pub maps_patched: Vec<PathBuf>,
    pub backups: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// Add `lang` (e.g. `es`) with display `label` (e.g. `Español`) to a game's
/// language menu plumbing so Iavra packs + Options + boot choices can select it.
///
/// Creates `*.bak-locust` siblings for every file written.
pub fn register_language(
    game_root: &Path,
    lang: &str,
    label: &str,
) -> Result<RegisterLanguageReport> {
    register_language_with_db(game_root, lang, label, None)
}

/// Register a language and extend only a matching Add recording, when supplied.
/// Files and their recording commit under the same game lock. Games without a
/// matching Add recording keep the standalone registration behavior.
pub fn register_language_with_db(
    game_root: &Path,
    lang: &str,
    label: &str,
    db: Option<&Database>,
) -> Result<RegisterLanguageReport> {
    let lang = lang.trim();
    let label = label.trim();
    if lang.is_empty()
        || !lang
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(LocustError::ParseError {
            file: game_root.display().to_string(),
            message: format!("invalid language code: {lang:?}"),
        });
    }
    if label.is_empty() {
        return Err(LocustError::ParseError {
            file: game_root.display().to_string(),
            message: "language label must not be empty".to_string(),
        });
    }

    let lock = GameLock::acquire(game_root)?;
    ensure_no_pending_under_lock(&lock)?;

    let mut report = RegisterLanguageReport::default();
    let mut planned = Vec::new();

    let plugins = game_root.join("js").join("plugins.js");
    if plugins.is_file() {
        let (file, iavra, visu) = patch_plugins_js(&plugins, lang, label)?;
        // Keep the existing plugins.js backup/report behavior even on a no-op.
        planned.push(file);
        report.plugins_js = iavra || visu;
        report.iavra_languages = iavra;
        report.visumz_options = visu;
        if !iavra && !visu {
            report
                .notes
                .push("plugins.js present but no Iavra/VisuMZ language patterns matched".into());
        }
    } else {
        report
            .notes
            .push("js/plugins.js not found — skipped Iavra/VisuMZ patch".into());
    }

    let data_dir =
        RpgMakerMvPlugin::find_data_dir(game_root).unwrap_or_else(|| game_root.join("data"));
    if data_dir.is_dir() {
        let maps = patch_language_choice_maps(&data_dir, lang, label)?;
        for file in maps {
            report.maps_patched.push(file.path.clone());
            planned.push(file);
        }
    }

    if !report.plugins_js && report.maps_patched.is_empty() {
        // Idempotent: already registered, or no hooks. Callers treat empty change as OK.
        report.notes.push(
            "nothing changed — already registered, or game has no Iavra/VisuMZ/Map language hooks"
                .into(),
        );
    }

    // Every read, decode and parse has succeeded before the first backup/write.
    report.backups = write_planned_files(&lock, game_root, &planned, db, lang)?;
    Ok(report)
}

struct PlannedFile {
    path: PathBuf,
    original: Vec<u8>,
    content: Option<String>,
}

fn write_planned_files(
    lock: &GameLock,
    game_root: &Path,
    planned: &[PlannedFile],
    db: Option<&Database>,
    lang: &str,
) -> Result<Vec<PathBuf>> {
    let mut created_backups = Vec::new();
    let result = (|| {
        let mut backups = Vec::new();
        let mut files = Vec::new();
        for file in planned {
            let relative = file.path.strip_prefix(game_root).map_err(|_| {
                LocustError::InjectionError("language registration path escaped game root".into())
            })?;
            locust_core::patch::zipsec::ensure_no_links(lock.root(), relative)?;
            if std::fs::read(&file.path)? != file.original {
                return Err(LocustError::InjectionError(format!(
                    "game changed while preparing language registration: {}",
                    file.path.display()
                )));
            }
            if let Some(content) = &file.content {
                files.push((relative.to_owned(), content.as_bytes().to_vec()));
            }
        }
        let recording = if files.is_empty() {
            None
        } else {
            db.map(|db| add_recording_under_lock(lock, db, lang))
                .transpose()?
                .flatten()
        };
        let registration_backup = if let Some(recording) = &recording {
            // Capture these exact pre-registration bytes, even if a map was
            // edited after Add. Keep the Add backup for its own original files.
            let db_path = db.expect("recording requires a database").path();
            let storage = recording
                .pristine_backup
                .as_ref()
                .and_then(|backup| backup.storage_root.clone())
                .unwrap_or_else(|| {
                    db_path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or_else(|| lock.root().parent().unwrap())
                        .join("locust-registration-backups")
                });
            let manager = locust_core::backup::BackupManager::new(std::path::absolute(storage)?);
            let originals_match = |tree: &Path| -> Result<bool> {
                for file in planned.iter().filter(|file| file.content.is_some()) {
                    let relative = file.path.strip_prefix(game_root).map_err(|_| {
                        LocustError::InjectionError(
                            "language registration path escaped game root".into(),
                        )
                    })?;
                    if std::fs::read(tree.join(relative)).ok().as_deref() != Some(&file.original) {
                        return Ok(false);
                    }
                }
                Ok(true)
            };
            let reusable = recording.pristine_backup.as_ref().filter(|backup| {
                backup.storage_root.as_deref() == Some(manager.root())
                    && locust_core::database::paths_identical(&backup.source_path, lock.root())
            });
            let reusable = match reusable {
                Some(backup)
                    if manager.with_pristine_tree(
                        &backup.id,
                        &backup.source_path,
                        originals_match,
                    )? =>
                {
                    Some(backup)
                }
                _ => None,
            };
            if let Some(backup) = reusable {
                Some(backup.clone())
            } else {
                let backup = manager.create_backup(lock.root())?;
                if !manager.with_pristine_tree(&backup.id, &backup.source_path, originals_match)? {
                    return Err(LocustError::InjectionError(
                        "game changed while backing up language registration".into(),
                    ));
                }
                Some(RecordedBackup {
                    id: backup.id,
                    source_path: backup.source_path,
                    storage_root: Some(manager.root().to_owned()),
                })
            }
        } else {
            None
        };
        for file in planned {
            backups.push(backup_file(&file.path, &mut created_backups)?);
        }
        write_files_and_record_under_lock(
            lock,
            &files,
            "register-language",
            |_index| {
                #[cfg(test)]
                tests::after_install(_index);
                Ok(())
            },
            || {
                if let (Some(db), Some(recording), Some(backup)) =
                    (db, &recording, &registration_backup)
                {
                    let written: Vec<_> =
                        files.iter().map(|(rel, _)| lock.root().join(rel)).collect();
                    db.extend_injection_with_registration_backup(recording, &written, backup)?;
                }
                Ok(())
            },
        )?;
        Ok(backups)
    })();
    if result.is_err() {
        // The adapter has rolled back under this same lock. Retain compatibility
        // backups if rollback failed or an external editor changed an original.
        if ensure_no_pending_under_lock(lock).is_ok()
            && planned
                .iter()
                .all(|file| std::fs::read(&file.path).is_ok_and(|bytes| bytes == file.original))
        {
            for backup in created_backups.into_iter().rev() {
                if let Err(error) = std::fs::remove_file(&backup) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::error!(path = %backup.display(), %error, "language registration backup cleanup failed");
                    }
                }
            }
        }
    }
    result
}

fn backup_file(path: &Path, created_backups: &mut Vec<PathBuf>) -> Result<PathBuf> {
    let bak = path.parent().unwrap_or(path).join(format!(
        "{}.bak-locust",
        path.file_name().unwrap().to_string_lossy()
    ));
    if !bak.exists() {
        let parent = path.parent().unwrap();
        locust_core::patch::zipsec::ensure_no_links(parent, Path::new(bak.file_name().unwrap()))?;
        let stage = locust_core::patch::stream::StagingDir::create_prepared(parent)?;
        let temp = stage.child("backup");
        let mut output = stage.create_file("backup")?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut output)?;
        output.sync_all()?;
        drop(output);
        std::fs::rename(temp, &bak)?;
        created_backups.push(bak.clone());
    }
    Ok(bak)
}

fn patch_plugins_js(path: &Path, lang: &str, label: &str) -> Result<(PlannedFile, bool, bool)> {
    let mut raw = std::fs::read_to_string(path)?;
    let original = raw.as_bytes().to_vec();
    let mut iavra = false;
    let mut visu = false;

    // --- Iavra Languages: "jp, en, zh" ---
    if let Some(new_raw) = patch_iavra_languages_param(&raw, lang) {
        raw = new_raw;
        iavra = true;
    }
    if let Some(new_raw) = patch_iavra_labels_param(&raw, lang, label) {
        raw = new_raw;
        iavra = true;
    }

    // --- VisuMZ: const langs = ['jp', 'en', 'zh'] ---
    let langs_pat = "langs = ['";
    if raw.contains(langs_pat) {
        // Replace any langs = ['a', 'b', ...] that lacks lang
        let before = raw.clone();
        raw = extend_js_string_array_literal(&raw, "langs = ", lang);
        if raw != before {
            visu = true;
        }
    }

    // VisuMZ length clamp used optionsCoreFonts.length - rewrite near IAVRA/langs
    if raw.contains("langs = [") && raw.contains("optionsCoreFonts.length - 1") {
        let before = raw.clone();
        raw = rewrite_lang_length_clamps(&raw);
        if raw != before {
            visu = true;
        }
    }

    // FontFaces:arraystr for cycle length (jp,en,tc → +lang code)
    if let Some(new_raw) = extend_fontfaces_array(&raw, lang) {
        raw = new_raw;
        visu = true;
    }

    // DrawJS: append label if Language option draws 日本語/English/中文 style trio
    if let Some(new_raw) = append_language_draw_label(&raw, lang, label) {
        raw = new_raw;
        visu = true;
    }

    // ConfigManager.lang sync after IAVRA set (UltraHUD corners)
    if raw.contains("IAVRA.MasterLocalization.I18N.language = langs[value]")
        && !raw.contains("ConfigManager.lang = value")
    {
        // Insert with same newline escaping as neighboring statements when possible
        if raw.contains("langs[value];") {
            // Only add once near ProcessOk-style blocks — optional, skip if fragile
        }
    }

    Ok((
        PlannedFile {
            path: path.to_owned(),
            original,
            content: (iavra || visu).then_some(raw),
        },
        iavra,
        visu,
    ))
}

fn patch_iavra_languages_param(raw: &str, lang: &str) -> Option<String> {
    // "Languages":"jp, en, zh"
    let key = "\"Languages\":\"";
    let start = raw.find(key)? + key.len();
    let end = raw[start..].find('"')? + start;
    let list = &raw[start..end];
    let codes: Vec<&str> = list
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if codes.contains(&lang) {
        return None;
    }
    let mut new_list = list.to_string();
    if !new_list.is_empty() && !new_list.ends_with(' ') {
        // keep ", " style if present
        if list.contains(", ") {
            new_list.push_str(", ");
        } else if list.contains(',') {
            new_list.push(',');
        } else {
            new_list.push_str(", ");
        }
    }
    new_list.push_str(lang);
    Some(format!("{}{}{}", &raw[..start], new_list, &raw[end..]))
}

fn patch_iavra_labels_param(raw: &str, lang: &str, label: &str) -> Option<String> {
    // "Language Labels":"en:English, jp:日本語, zh:中文"
    let key = "\"Language Labels\":\"";
    let start = raw.find(key)? + key.len();
    let end = raw[start..].find('"')? + start;
    let list = &raw[start..end];
    if list
        .split(',')
        .any(|p| p.trim().starts_with(&format!("{lang}:")))
    {
        return None;
    }
    let mut new_list = list.to_string();
    if !new_list.is_empty() {
        // Iavra splits on ',' and trims, so a uniform ", " is safe whatever
        // separator style the existing list used.
        new_list.push_str(", ");
    }
    new_list.push_str(&format!("{lang}:{}", js_json_string_escape(label)));
    Some(format!("{}{}{}", &raw[..start], new_list, &raw[end..]))
}

fn js_json_string_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Extend JS array literals after a marker, e.g. `langs = ['jp', 'en', 'zh']`.
fn extend_js_string_array_literal(raw: &str, marker: &str, lang: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 16);
    let mut rest = raw;
    let quoted = format!("'{lang}'");
    let dquoted = format!("\"{lang}\"");
    while let Some(idx) = rest.find(marker) {
        out.push_str(&rest[..idx]);
        let after_marker = &rest[idx..];
        // Find opening [
        let Some(ob) = after_marker.find('[') else {
            out.push_str(marker);
            rest = &rest[idx + marker.len()..];
            continue;
        };
        let Some(cb) = after_marker[ob..].find(']') else {
            out.push_str(marker);
            rest = &rest[idx + marker.len()..];
            continue;
        };
        let arr = &after_marker[ob..ob + cb + 1];
        out.push_str(&after_marker[..ob]);
        if arr.contains(&quoted) || arr.contains(&dquoted) {
            out.push_str(arr);
        } else {
            // Insert before ]
            let inner = &arr[1..arr.len() - 1];
            let use_double = inner.contains('"') && !inner.contains('\'');
            let item = if use_double {
                format!("\"{lang}\"")
            } else {
                format!("'{lang}'")
            };
            if inner.trim().is_empty() {
                out.push('[');
                out.push_str(&item);
                out.push(']');
            } else {
                let sep = if inner.contains(", ") { ", " } else { "," };
                out.push('[');
                out.push_str(inner.trim_end());
                out.push_str(sep);
                out.push_str(&item);
                out.push(']');
            }
        }
        rest = &after_marker[ob + cb + 1..];
    }
    out.push_str(rest);
    out
}

fn floor_char_boundary(raw: &str, mut index: usize) -> usize {
    while !raw.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn rewrite_lang_length_clamps(raw: &str) -> String {
    // When a langs = [...] array is nearby, optionsCoreFonts.length - 1 is wrong.
    // Replace with (langs.length - 1) which VisuMZ evaluates as JS.
    let mut out = raw.to_string();
    let needle = "TextManager.optionsCoreFonts.length - 1";
    let mut search_from = 0;
    while let Some(rel) = out[search_from..].find(needle) {
        let pos = search_from + rel;
        let window_start = floor_char_boundary(&out, pos.saturating_sub(600));
        let window = &out[window_start..pos + needle.len()];
        if window.contains("langs")
            || window.contains("IAVRA")
            || window.contains("MasterLocalization")
        {
            out.replace_range(pos..pos + needle.len(), "(langs.length - 1)");
            search_from = pos + "(langs.length - 1)".len();
        } else {
            search_from = pos + needle.len();
        }
    }
    out
}

fn extend_fontfaces_array(raw: &str, lang: &str) -> Option<String> {
    let key = "FontFaces:arraystr";
    let fi = raw.find(key)?;
    let region_end = floor_char_boundary(raw, (fi + 200).min(raw.len()));
    let region = &raw[fi..region_end];
    if region.contains(&format!("\"{lang}\""))
        || region.contains(&format!("\\\"{lang}\\\""))
        || region.contains(&format!("'{lang}'"))
    {
        return None;
    }
    // Find first [ after FontFaces and extend
    let abs_open = fi + region.find('[')?;
    let abs_close = fi + region.find(']')?;
    let arr = &raw[abs_open..=abs_close];
    if arr.contains(lang) {
        return None;
    }
    // Detect quote style inside array
    let insert = if arr.contains("\\\"") {
        format!(",\\\"{lang}\\\"")
    } else if arr.contains("\"") {
        format!(",\"{lang}\"")
    } else {
        format!(",'{lang}'")
    };
    let mut new_raw = raw.to_string();
    new_raw.insert_str(abs_close, &insert);
    Some(new_raw)
}

fn append_language_draw_label(raw: &str, _lang: &str, label: &str) -> Option<String> {
    // Look for drawText('中文' ... ) as last of the common trio; append Español-style draw.
    // Also accept drawText("中文"
    let markers = ["drawText('中文'", "drawText(\"中文\"", "drawText('中文'"];
    let mut hit = None;
    for m in markers {
        if let Some(i) = raw.find(m) {
            hit = Some((i, m));
            break;
        }
    }
    let (zi, _m) = hit?;
    if raw.contains(&format!("drawText('{label}'"))
        || raw.contains(&format!("drawText(\"{label}\""))
    {
        return None;
    }
    // Find end of this drawText call — first ')' after zi that closes it (simple scan)
    let slice = &raw[zi..];
    let mut depth = 0i32;
    let mut end = None;
    for (i, ch) in slice.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(zi + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    // Newline sequence before this drawText
    let before = &raw[floor_char_boundary(raw, zi.saturating_sub(40))..zi];
    let nl = if before.contains("\\\\\\\\\\\\\\\\n") {
        // keep short relative escape — copy last backslash-n run
        let mut run = "\\n";
        if let Some(idx) = before.rfind('n') {
            let mut s = idx;
            while s > 0 && before.as_bytes()[s - 1] == b'\\' {
                s -= 1;
            }
            run = &before[s..=idx];
        }
        run
    } else {
        "\\n"
    };

    // Quote style for center from the 中文 call
    let center_q = if raw[zi..end].contains("\\\"center\\\"") {
        "\\\"center\\\""
    } else {
        "\"center\""
    };
    let label_lit = if raw[zi..end].contains('\'') {
        format!("'{}'", label.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        format!("\"{}\"", label.replace('\\', "\\\\").replace('"', "\\\""))
    };

    let insert = format!(
        "{nl}this.changePaintOpacity((value==3));{nl}const fx4 = rect.x + halfWidth + (segment * 3);{nl}this.drawText({label_lit}, fx4, rect.y, segment, {center_q})"
    );

    // Also ensure segment uses / 4 if still / 3 near Language draw
    let mut new_raw = raw.to_string();
    // Prefer local halfWidth / 3 → / 4 once
    if let Some(hw) = new_raw.find("halfWidth / 3") {
        if hw + 200 > zi || zi.saturating_sub(hw) < 2000 {
            new_raw.replace_range(hw..hw + "halfWidth / 3".len(), "halfWidth / 4");
        }
    }
    // Recompute end after possible length change — re-find marker
    let zi2 = new_raw
        .find("drawText('中文'")
        .or_else(|| new_raw.find("drawText(\"中文\""))?;
    let slice2 = &new_raw[zi2..];
    let mut depth = 0i32;
    let mut end2 = None;
    for (i, ch) in slice2.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end2 = Some(zi2 + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end2 = end2?;
    new_raw.insert_str(end2, &insert);
    Some(new_raw)
}

fn patch_language_choice_maps(
    data_dir: &Path,
    lang: &str,
    label: &str,
) -> Result<Vec<PlannedFile>> {
    let mut changed = Vec::new();
    let rd = match std::fs::read_dir(data_dir) {
        Ok(r) => r,
        Err(_) => return Ok(changed),
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let stem = name
            .strip_suffix(".jsono")
            .or_else(|| name.strip_suffix(".json"))
            .unwrap_or(name);
        let lower = stem.to_ascii_lowercase();
        if !(lower.starts_with("map") && lower[3..].chars().all(|c| c.is_ascii_digit())) {
            continue;
        }
        if let Some(file) = patch_one_map(&path, lang, label)? {
            changed.push(file);
        }
    }
    Ok(changed)
}

fn patch_one_map(path: &Path, lang: &str, label: &str) -> Result<Option<PlannedFile>> {
    let original = std::fs::read(path)?;
    let (raw, _) = EncodingDetector::detect_and_decode(&original)?;
    let text = if path.extension().and_then(|e| e.to_str()) == Some("jsono") {
        let units =
            lz_str::decompress_from_base64(raw.trim()).ok_or_else(|| LocustError::ParseError {
                file: path.display().to_string(),
                message: "failed to decompress map .jsono".into(),
            })?;
        String::from_utf16_lossy(&units)
    } else {
        raw
    };

    let mut map: serde_json::Value = serde_json::from_str(&text)?;
    let mut any = false;

    let Some(events) = map.get_mut("events").and_then(|v| v.as_array_mut()) else {
        return Ok(None);
    };

    for ev in events.iter_mut() {
        if ev.is_null() {
            continue;
        }
        let Some(pages) = ev.get_mut("pages").and_then(|v| v.as_array_mut()) else {
            continue;
        };
        for page in pages.iter_mut() {
            let Some(list) = page.get_mut("list").and_then(|v| v.as_array_mut()) else {
                continue;
            };
            if patch_event_list(list, lang, label) {
                any = true;
            }
        }
    }

    if !any {
        return Ok(None);
    }

    let out = serde_json::to_string(&map)?;
    let encoded = if path.extension().and_then(|e| e.to_str()) == Some("jsono") {
        lz_str::compress_to_base64(&out)
    } else {
        serde_json::to_string_pretty(&map)?
    };
    Ok(Some(PlannedFile {
        path: path.to_owned(),
        original,
        content: Some(encoded),
    }))
}

fn looks_like_language_choices(choices: &[String]) -> bool {
    if choices.len() < 2 || choices.len() > 8 {
        return false;
    }
    let joined = choices.join(" ").to_ascii_lowercase();
    let has_en = choices.iter().any(|c| {
        let l = c.to_ascii_lowercase();
        l == "english" || l == "en" || l.contains("english")
    });
    let has_cjk_menu = choices.iter().any(|c| {
        c.contains('日')
            || c.contains('中')
            || c.contains('韩')
            || c.contains('語')
            || c.contains('文')
    });
    // Classic jp/en/zh picker
    (has_en && has_cjk_menu)
        || (joined.contains("english") && (joined.contains("日本語") || joined.contains("中文")))
}

fn event_code(cmd: &serde_json::Value) -> u64 {
    cmd.get("code").and_then(|c| c.as_u64()).unwrap_or(0)
}

fn event_indent(cmd: &serde_json::Value) -> i64 {
    cmd.get("indent")
        .and_then(|c| c.as_i64().or_else(|| c.as_u64().map(|u| u as i64)))
        .unwrap_or(0)
}

fn skip_choice_branch_body(list: &[serde_json::Value], mut j: usize, indent: i64) -> usize {
    while j < list.len() {
        let c2 = event_code(&list[j]);
        let i2 = event_indent(&list[j]);
        if i2 < indent {
            break;
        }
        if i2 == indent && (c2 == 402 || c2 == 403 || c2 == 404) {
            break;
        }
        j += 1;
    }
    j
}

fn is_english_choice_branch(cmd: &serde_json::Value) -> bool {
    let idx = cmd
        .get("parameters")
        .and_then(|p| p.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let name = cmd
        .get("parameters")
        .and_then(|p| p.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    idx == 1 || name.eq_ignore_ascii_case("english") || name.eq_ignore_ascii_case("en")
}

fn collect_choice_branches(
    list: &[serde_json::Value],
    i_show: usize,
) -> (Vec<(usize, usize)>, Option<usize>) {
    let indent = event_indent(&list[i_show]);
    let mut j = i_show + 1;
    let mut branches: Vec<(usize, usize)> = Vec::new();
    let mut end404 = None;
    while j < list.len() {
        let c = event_code(&list[j]);
        let ind = event_indent(&list[j]);
        if ind < indent {
            break;
        }
        if ind == indent && c == 404 {
            end404 = Some(j);
            break;
        }
        if ind == indent && c == 402 {
            let start = j;
            j = skip_choice_branch_body(list, j + 1, indent);
            branches.push((start, j));
            continue;
        }
        if ind == indent && c == 403 {
            j = skip_choice_branch_body(list, j + 1, indent);
            continue;
        }
        j += 1;
    }
    (branches, end404)
}

fn script_param_mut(cmd: &mut serde_json::Value) -> Option<&mut String> {
    cmd.get_mut("parameters")
        .and_then(|p| p.as_array_mut())
        .and_then(|a| a.first_mut())
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        })
}

fn clone_usable_lang_branch(
    list: &[serde_json::Value],
    start: usize,
    end: usize,
    lang: &str,
    label: &str,
    new_index: i64,
) -> Option<Vec<serde_json::Value>> {
    let mut new_cmds: Vec<serde_json::Value> = list[start..end].to_vec();
    if let Some(first) = new_cmds.first_mut() {
        if let Some(params) = first.get_mut("parameters").and_then(|p| p.as_array_mut()) {
            if !params.is_empty() {
                params[0] = serde_json::json!(new_index);
            }
            if params.len() > 1 {
                params[1] = serde_json::Value::String(label.to_string());
            }
        }
    }
    let mut recognized = false;
    for cmd in &mut new_cmds {
        let c = event_code(cmd);
        if c != 355 && c != 655 {
            continue;
        }
        let Some(s) = cmd
            .get("parameters")
            .and_then(|p| p.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        match rewrite_lang_script(&s, lang, new_index as i32) {
            LangScriptRewrite::Refuse => return None,
            LangScriptRewrite::Unrelated(_) => {}
            LangScriptRewrite::Rewritten(rewritten) => {
                recognized = true;
                if let Some(slot) = script_param_mut(cmd) {
                    *slot = rewritten;
                }
            }
        }
    }
    if recognized {
        Some(new_cmds)
    } else {
        None
    }
}

fn patch_event_list(list: &mut Vec<serde_json::Value>, lang: &str, label: &str) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < list.len() {
        if event_code(&list[i]) != 102 {
            i += 1;
            continue;
        }
        let Some(choices) = list[i]
            .get("parameters")
            .and_then(|p| p.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_array())
        else {
            i += 1;
            continue;
        };
        let choice_strs: Vec<String> = choices
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !looks_like_language_choices(&choice_strs) {
            i += 1;
            continue;
        }
        if choice_strs
            .iter()
            .any(|c| c == label || c.eq_ignore_ascii_case(lang))
        {
            i += 1;
            continue;
        }

        let new_index = choice_strs.len() as i64;
        let (branches, end404) = collect_choice_branches(list, i);
        let Some(insert_at) = end404 else {
            i += 1;
            continue;
        };

        let en_range = branches.iter().copied().find(|(start, _)| {
            list.get(*start)
                .map(is_english_choice_branch)
                .unwrap_or(false)
        });
        let mut template = None;
        for range in en_range.into_iter().chain(branches.iter().copied()) {
            if let Some(cmds) =
                clone_usable_lang_branch(list, range.0, range.1, lang, label, new_index)
            {
                template = Some(cmds);
                break;
            }
        }
        let Some(new_cmds) = template else {
            i += 1;
            continue;
        };

        let Some(choices_val) = list[i]
            .get_mut("parameters")
            .and_then(|p| p.as_array_mut())
            .and_then(|a| a.get_mut(0))
        else {
            i += 1;
            continue;
        };
        let Some(choices) = choices_val.as_array_mut() else {
            i += 1;
            continue;
        };
        choices.push(serde_json::Value::String(label.to_string()));
        for (k, cmd) in new_cmds.into_iter().enumerate() {
            list.insert(insert_at + k, cmd);
        }
        changed = true;
        i += 1;
    }
    changed
}

#[derive(Debug, PartialEq, Eq)]
enum LangScriptRewrite {
    Unrelated(String),
    Rewritten(String),
    Refuse,
}

fn skip_ws_bytes(s: &str, mut i: usize) -> usize {
    let b = s.as_bytes();
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn at_ident_start(s: &str, i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = s.as_bytes()[i - 1];
    !(prev.is_ascii_alphanumeric() || prev == b'_')
}

fn exact_config_manager_lang_at(s: &str, i: usize) -> bool {
    const NEEDLE: &str = "ConfigManager.lang";
    if i + NEEDLE.len() > s.len() || !s[i..].starts_with(NEEDLE) || !at_ident_start(s, i) {
        return false;
    }
    let after = i + NEEDLE.len();
    if after < s.len() {
        let n = s.as_bytes()[after];
        if n.is_ascii_alphanumeric() || n == b'_' {
            return false;
        }
    }
    true
}

fn skip_comment_or_string(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    if i >= b.len() {
        return None;
    }
    if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
        let mut j = i + 2;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        return Some(j);
    }
    if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
        let mut j = i + 2;
        while j + 1 < b.len() && !(b[j] == b'*' && b[j + 1] == b'/') {
            j += 1;
        }
        if j + 1 < b.len() {
            j += 2;
        } else {
            j = b.len();
        }
        return Some(j);
    }
    if b[i] == b'"' || b[i] == b'\'' || b[i] == b'`' {
        let q = b[i];
        let mut j = i + 1;
        while j < b.len() {
            if b[j] == b'\\' {
                j += 1;
                if j < b.len() {
                    j += 1;
                }
                continue;
            }
            if b[j] == q {
                j += 1;
                break;
            }
            j += 1;
        }
        return Some(j);
    }
    None
}

fn scan_ascii_int(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    if i >= b.len() {
        return None;
    }
    let mut j = i;
    if b[j] == b'-' || b[j] == b'+' {
        j += 1;
    }
    if j >= b.len() || !b[j].is_ascii_digit() {
        return None;
    }
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    if j < b.len() {
        let n = b[j];
        if n == b'.' || n == b'e' || n == b'E' || n.is_ascii_alphabetic() || n == b'_' {
            return None;
        }
    }
    Some(j)
}

fn is_simple_assign_eq(s: &str, i: usize) -> bool {
    let b = s.as_bytes();
    if i >= b.len() || b[i] != b'=' {
        return false;
    }
    if i > 0 {
        let p = b[i - 1];
        if matches!(
            p,
            b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^'
        ) {
            return false;
        }
    }
    if i + 1 < b.len() && (b[i + 1] == b'=' || b[i + 1] == b'>') {
        return false;
    }
    true
}

fn skip_quoted_payload(s: &str, mut i: usize, q: u8) -> Option<usize> {
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'\\' {
            i += 1;
            if i < b.len() {
                i += 1;
            }
            continue;
        }
        if b[i] == q {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn rewrite_lang_script(script: &str, lang: &str, index: i32) -> LangScriptRewrite {
    let mut out = String::with_capacity(script.len() + lang.len() + 8);
    let mut i = 0;
    let mut saw = false;
    while i < script.len() {
        if let Some(n) = skip_comment_or_string(script, i) {
            out.push_str(&script[i..n]);
            i = n;
            continue;
        }
        if script[i..].starts_with("I18N.language") && at_ident_start(script, i) {
            out.push_str("I18N.language");
            i += "I18N.language".len();
            let ws1 = i;
            i = skip_ws_bytes(script, i);
            out.push_str(&script[ws1..i]);
            if is_simple_assign_eq(script, i) {
                out.push('=');
                i += 1;
                let ws2 = i;
                i = skip_ws_bytes(script, i);
                out.push_str(&script[ws2..i]);
                if i < script.len()
                    && (script.as_bytes()[i] == b'"' || script.as_bytes()[i] == b'\'')
                {
                    let q = script.as_bytes()[i];
                    out.push(q as char);
                    i += 1;
                    let Some(close) = skip_quoted_payload(script, i, q) else {
                        return LangScriptRewrite::Refuse;
                    };
                    i = close + 1;
                    out.push_str(lang);
                    out.push(q as char);
                    saw = true;
                    continue;
                }
                return LangScriptRewrite::Refuse;
            }
            continue;
        }
        if exact_config_manager_lang_at(script, i) {
            out.push_str("ConfigManager.lang");
            i += "ConfigManager.lang".len();
            let ws1 = i;
            i = skip_ws_bytes(script, i);
            out.push_str(&script[ws1..i]);
            if is_simple_assign_eq(script, i) {
                out.push('=');
                i += 1;
                let ws2 = i;
                i = skip_ws_bytes(script, i);
                out.push_str(&script[ws2..i]);
                let Some(end) = scan_ascii_int(script, i) else {
                    return LangScriptRewrite::Refuse;
                };
                i = end;
                out.push_str(&index.to_string());
                saw = true;
                continue;
            }
            return LangScriptRewrite::Refuse;
        }
        let ch = match script[i..].chars().next() {
            Some(c) => c,
            None => break,
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    if saw {
        LangScriptRewrite::Rewritten(out)
    } else {
        LangScriptRewrite::Unrelated(script.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    thread_local! {
        static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    }

    pub(super) fn after_install(index: usize) {
        if STOP_AFTER.get() == Some(index) {
            // Exit without unwinding, so neither rollback nor guard destructors run.
            std::process::exit(86);
        }
    }

    fn crash_fixture(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fs::create_dir_all(root.join("js")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/System.json"), br#"{"gameTitle":"T"}"#).unwrap();
        let plugins = br#"var $plugins = [{"parameters":{"Languages":"jp, en, zh"}}];"#;
        let map = serde_json::json!({"events": [null, {"pages": [{"list": [
            {"code":102,"indent":0,"parameters":[["日本語","ENGLISH","中文"],-1,0,1,0]},
            {"code":402,"indent":0,"parameters":[1,"ENGLISH"]},
            {"code":355,"indent":1,"parameters":["I18N.language = 'en';"]},
            {"code":0,"indent":1,"parameters":[]},
            {"code":404,"indent":0,"parameters":[]}
        ]}]}]});
        let files = vec![
            (root.join("js/plugins.js"), plugins.to_vec()),
            (root.join("data/Map001.json"), map.to_string().into_bytes()),
            (
                root.join("data/Map002.jsono"),
                lz_str::compress_to_base64(&map.to_string()).into_bytes(),
            ),
        ];
        for (path, bytes) in &files {
            fs::write(path, bytes).unwrap();
        }
        files
    }

    #[test]
    fn register_lang_crash_child() {
        let Some(root) = std::env::var_os("LOCUST_REGISTER_LANG_CRASH_GAME") else {
            return;
        };
        STOP_AFTER.set(Some(
            std::env::var("LOCUST_REGISTER_LANG_CRASH_AFTER")
                .unwrap()
                .parse()
                .unwrap(),
        ));
        register_language(Path::new(&root), "es", "Español").unwrap();
        panic!("crash hook was not reached");
    }

    #[test]
    fn register_lang_crash_recovers_all_originals() {
        for stop_after in [0, 1] {
            let dir = tempfile::tempdir().unwrap();
            let originals = crash_fixture(dir.path());
            let stale_backup = dir.path().join("js/plugins.js.bak-locust");
            fs::write(&stale_backup, b"older user backup").unwrap();
            let expected_plugins = patch_plugins_js(&originals[0].0, "es", "Español")
                .unwrap()
                .0
                .content
                .unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "rpgmaker_lang::tests::register_lang_crash_child",
                    "--nocapture",
                ])
                .env("LOCUST_REGISTER_LANG_CRASH_GAME", dir.path())
                .env("LOCUST_REGISTER_LANG_CRASH_AFTER", stop_after.to_string())
                .output()
                .unwrap();
            assert_eq!(
                child.status.code(),
                Some(86),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            assert_eq!(
                fs::read(&originals[0].0).unwrap(),
                expected_plugins.as_bytes()
            );
            if stop_after == 0 {
                for (path, bytes) in &originals[1..] {
                    assert_eq!(&fs::read(path).unwrap(), bytes);
                }
            }
            let pending = locust_core::injection_transaction::status(dir.path())
                .unwrap()
                .pending
                .expect("an interrupted registration must be visible to Injection Recovery");
            assert_eq!(pending.changed_files, originals.len());
            assert!(pending.conflicts.is_empty());
            assert!(register_language(dir.path(), "fr", "Français").is_err());
            let recovered =
                locust_core::injection_transaction::recover(dir.path(), Default::default())
                    .unwrap();
            assert_eq!(recovered.restored, stop_after + 1);
            for (path, bytes) in &originals {
                assert_eq!(&fs::read(path).unwrap(), bytes);
            }
            assert_eq!(fs::read(&stale_backup).unwrap(), b"older user backup");
            assert!(locust_core::injection_transaction::status(dir.path())
                .unwrap()
                .pending
                .is_none());
            register_language(dir.path(), "es", "Español").unwrap();
        }
    }

    #[test]
    fn register_lang_replaces_live_files_without_truncating_open_handles() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let originals = crash_fixture(dir.path());
        let mut handles: Vec<_> = originals
            .iter()
            .map(|(path, _)| fs::File::open(path).unwrap())
            .collect();
        let report = register_language(dir.path(), "es", "Español").unwrap();
        assert!(report.plugins_js);
        assert_eq!(report.maps_patched.len(), 2);
        for ((path, original), handle) in originals.iter().zip(&mut handles) {
            let mut observed = Vec::new();
            handle.read_to_end(&mut observed).unwrap();
            assert_eq!(
                &observed,
                original,
                "an open reader must retain the complete old file: {}",
                path.display()
            );
            assert_ne!(&fs::read(path).unwrap(), original);
        }
        assert!(locust_core::injection_transaction::game_status(dir.path())
            .unwrap()
            .injections
            .is_empty());
    }

    fn write_jsono(path: &Path, json: &str) {
        fs::write(path, lz_str::compress_to_base64(json)).unwrap();
    }

    #[test]
    fn test_patch_iavra_languages_param() {
        let raw = r#"{"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}"#;
        let out = patch_iavra_languages_param(raw, "es").unwrap();
        assert!(out.contains("jp, en, zh, es"));
        assert!(patch_iavra_languages_param(&out, "es").is_none());
        let out2 = patch_iavra_labels_param(&out, "es", "Español").unwrap();
        assert!(out2.contains("es:Español"));
    }

    #[test]
    fn test_extend_langs_array() {
        let raw = "const langs = ['jp', 'en', 'zh'];\nfoo";
        let out = extend_js_string_array_literal(raw, "langs = ", "es");
        assert!(out.contains("'es'"), "{out}");
        assert!(out.contains("'zh'"));
    }

    #[test]
    fn test_register_language_map_choice() {
        let dir = std::env::temp_dir().join(format!("locust_reglang_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(dir.join("js")).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(dir.join("js").join("rmmz_core.js"), "// mz").unwrap();
        fs::write(
            dir.join("js").join("plugins.js"),
            r#"var $plugins = [{"name":"Iavra_MZ_Localization_byNeomaStudio","status":true,"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}];
const langs = ['jp', 'en', 'zh'];
const length = TextManager.optionsCoreFonts.length - 1;
IAVRA.MasterLocalization.I18N.language = langs[value];
this.drawText('中文', fx3, rect.y, segment, "center")
"#,
        )
        .unwrap();

        let map = serde_json::json!({
            "events": [
                null,
                {
                    "pages": [{
                        "list": [
                            {"code": 102, "indent": 0, "parameters": [["日本語", "ENGLISH", "中文"], -1, 0, 1, 0]},
                            {"code": 402, "indent": 0, "parameters": [0, "日本語"]},
                            {"code": 355, "indent": 1, "parameters": ["IAVRA.MasterLocalization.I18N.language = \"jp\";"]},
                            {"code": 655, "indent": 1, "parameters": ["ConfigManager.lang = 0;"]},
                            {"code": 0, "indent": 1, "parameters": []},
                            {"code": 402, "indent": 0, "parameters": [1, "ENGLISH"]},
                            {"code": 355, "indent": 1, "parameters": ["IAVRA.MasterLocalization.I18N.language = \"en\";"]},
                            {"code": 655, "indent": 1, "parameters": ["ConfigManager.lang = 1;"]},
                            {"code": 0, "indent": 1, "parameters": []},
                            {"code": 402, "indent": 0, "parameters": [2, "中文"]},
                            {"code": 355, "indent": 1, "parameters": ["IAVRA.MasterLocalization.I18N.language = \"zh\";"]},
                            {"code": 655, "indent": 1, "parameters": ["ConfigManager.lang = 2;"]},
                            {"code": 0, "indent": 1, "parameters": []},
                            {"code": 404, "indent": 0, "parameters": []},
                            {"code": 355, "indent": 0, "parameters": ["ConfigManager.language = IAVRA.MasterLocalization.I18N.language;"]},
                            {"code": 0, "indent": 0, "parameters": []}
                        ]
                    }]
                }
            ]
        });
        write_jsono(&data.join("Map012.jsono"), &map.to_string());
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"T","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
        );

        let report = register_language(&dir, "es", "Español").unwrap();
        assert!(report.iavra_languages || report.plugins_js);
        assert!(!report.maps_patched.is_empty(), "map should be patched");

        let plugins = fs::read_to_string(dir.join("js").join("plugins.js")).unwrap();
        assert!(plugins.contains("es"), "{plugins}");
        assert!(plugins.contains("Español") || plugins.contains("'es'"));

        let units = lz_str::decompress_from_base64(
            fs::read_to_string(data.join("Map012.jsono"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        let map_text = String::from_utf16_lossy(&units);
        assert!(map_text.contains("Español"), "{map_text}");
        assert!(
            map_text.contains("language = \\\"es\\\"") || map_text.contains("language = \"es\"")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_looks_like_language_choices() {
        assert!(looks_like_language_choices(&[
            "日本語".into(),
            "ENGLISH".into(),
            "中文".into()
        ]));
        assert!(!looks_like_language_choices(&["Yes".into(), "No".into()]));
    }
}

#[cfg(test)]
#[path = "rpgmaker_lang_fix_tests.rs"]
mod rpgmaker_lang_fix_tests;
