//! Register an extra UI language on deployed RPG Maker MV/MZ multi-lang titles.
//!
//! Handles the common Waterbear / Iavra + VisuMZ OptionsCore pattern and
//! title-map Show Choices language pickers (as in Elf-Goblin United Front).
//!
//! Not a full VisuMZ editor — best-effort string surgery with backups.

use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::encoding::EncodingDetector;

use crate::rpgmaker_mv::RpgMakerMvPlugin;

/// Report of what `register_language` changed on disk.
#[derive(Debug, Clone, Default)]
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
    let lang = lang.trim();
    let label = label.trim();
    if lang.is_empty() || !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
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

    let mut report = RegisterLanguageReport::default();

    let plugins = game_root.join("js").join("plugins.js");
    if plugins.is_file() {
        let backup = backup_file(&plugins)?;
        report.backups.push(backup);
        let (iavra, visu) = patch_plugins_js(&plugins, lang, label)?;
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
        for (path, bak) in maps {
            report.maps_patched.push(path);
            report.backups.push(bak);
        }
    }

    if !report.plugins_js && report.maps_patched.is_empty() {
        // Idempotent: already registered, or no hooks. Callers treat empty change as OK.
        report.notes.push(
            "nothing changed — already registered, or game has no Iavra/VisuMZ/Map language hooks"
                .into(),
        );
    }

    Ok(report)
}

fn backup_file(path: &Path) -> Result<PathBuf> {
    let bak = path.parent().unwrap_or(path).join(format!(
        "{}.bak-locust",
        path.file_name().unwrap().to_string_lossy()
    ));
    if !bak.exists() {
        std::fs::copy(path, &bak)?;
    }
    Ok(bak)
}

fn patch_plugins_js(path: &Path, lang: &str, label: &str) -> Result<(bool, bool)> {
    let mut raw = std::fs::read_to_string(path)?;
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

    if iavra || visu {
        std::fs::write(path, raw)?;
    }
    Ok((iavra, visu))
}

fn patch_iavra_languages_param(raw: &str, lang: &str) -> Option<String> {
    // "Languages":"jp, en, zh"
    let key = "\"Languages\":\"";
    let start = raw.find(key)? + key.len();
    let end = raw[start..].find('"')? + start;
    let list = &raw[start..end];
    let codes: Vec<&str> = list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if codes.iter().any(|c| *c == lang) {
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
    if list.split(',').any(|p| p.trim().starts_with(&format!("{lang}:"))) {
        return None;
    }
    let mut new_list = list.to_string();
    if !new_list.is_empty() {
        if list.contains(", ") {
            new_list.push_str(", ");
        } else {
            new_list.push_str(", ");
        }
    }
    new_list.push_str(&format!("{lang}:{label}"));
    Some(format!("{}{}{}", &raw[..start], new_list, &raw[end..]))
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

fn rewrite_lang_length_clamps(raw: &str) -> String {
    // When a langs = [...] array is nearby, optionsCoreFonts.length - 1 is wrong.
    // Replace with (langs.length - 1) which VisuMZ evaluates as JS.
    let mut out = raw.to_string();
    let needle = "TextManager.optionsCoreFonts.length - 1";
    let mut search_from = 0;
    while let Some(rel) = out[search_from..].find(needle) {
        let pos = search_from + rel;
        let window_start = pos.saturating_sub(600);
        let window = &out[window_start..pos + needle.len()];
        if window.contains("langs") || window.contains("IAVRA") || window.contains("MasterLocalization")
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
    let region_end = (fi + 200).min(raw.len());
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
    if raw.contains(&format!("drawText('{label}'")) || raw.contains(&format!("drawText(\"{label}\""))
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
    let before = &raw[zi.saturating_sub(40)..zi];
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
    } else if before.contains("\\n") {
        "\\n"
    } else {
        "\\n"
    };

    // Quote style for center from the 中文 call
    let center_q = if raw[zi..end].contains("\\\"center\\\"") {
        "\\\"center\\\""
    } else if raw[zi..end].contains("\"center\"") {
        "\"center\""
    } else {
        "\"center\""
    };
    let label_lit = if raw[zi..end].contains('\'') {
        format!("'{label}'")
    } else {
        format!("\"{label}\"")
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
    let zi2 = new_raw.find("drawText('中文'").or_else(|| new_raw.find("drawText(\"中文\""))?;
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
) -> Result<Vec<(PathBuf, PathBuf)>> {
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
        if patch_one_map(&path, lang, label)? {
            let bak = backup_file(&path)?;
            changed.push((path, bak));
        }
    }
    Ok(changed)
}

fn patch_one_map(path: &Path, lang: &str, label: &str) -> Result<bool> {
    let (raw, _) = EncodingDetector::read_file_auto(path)?;
    let text = if path.extension().and_then(|e| e.to_str()) == Some("jsono") {
        let units = lz_str::decompress_from_base64(raw.trim()).ok_or_else(|| {
            LocustError::ParseError {
                file: path.display().to_string(),
                message: "failed to decompress map .jsono".into(),
            }
        })?;
        String::from_utf16_lossy(&units)
    } else {
        raw
    };

    let mut map: serde_json::Value = serde_json::from_str(&text)?;
    let mut any = false;

    let Some(events) = map.get_mut("events").and_then(|v| v.as_array_mut()) else {
        return Ok(false);
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
        return Ok(false);
    }

    let out = serde_json::to_string(&map)?;
    if path.extension().and_then(|e| e.to_str()) == Some("jsono") {
        let enc = lz_str::compress_to_base64(&out);
        std::fs::write(path, enc)?;
    } else {
        std::fs::write(path, serde_json::to_string_pretty(&map)?)?;
    }
    Ok(true)
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
        c.contains('日') || c.contains('中') || c.contains('韩') || c.contains('語') || c.contains('文')
    });
    // Classic jp/en/zh picker
    (has_en && has_cjk_menu)
        || (joined.contains("english") && (joined.contains("日本語") || joined.contains("中文")))
}

fn patch_event_list(list: &mut Vec<serde_json::Value>, lang: &str, label: &str) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < list.len() {
        let code = list[i].get("code").and_then(|c| c.as_u64()).unwrap_or(0);
        if code != 102 {
            i += 1;
            continue;
        }
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
        let choice_strs: Vec<String> = choices
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !looks_like_language_choices(&choice_strs) {
            i += 1;
            continue;
        }
        if choice_strs.iter().any(|c| c == label || c.eq_ignore_ascii_case(lang)) {
            i += 1;
            continue;
        }

        let new_index = choices.len() as i64;
        choices.push(serde_json::Value::String(label.to_string()));
        changed = true;

        // Collect When branches until 404
        let mut j = i + 1;
        let mut branches: Vec<(usize, usize)> = Vec::new(); // (start, end exclusive)
        let mut en_branch: Option<(usize, usize)> = None;
        while j < list.len() {
            let c = list[j].get("code").and_then(|x| x.as_u64()).unwrap_or(0);
            if c == 402 {
                let start = j;
                let idx = list[j]
                    .get("parameters")
                    .and_then(|p| p.as_array())
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let name = list[j]
                    .get("parameters")
                    .and_then(|p| p.as_array())
                    .and_then(|a| a.get(1))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                j += 1;
                while j < list.len() {
                    let c2 = list[j].get("code").and_then(|x| x.as_u64()).unwrap_or(0);
                    if c2 == 402 || c2 == 403 || c2 == 404 {
                        break;
                    }
                    j += 1;
                }
                branches.push((start, j));
                if idx == 1
                    || name.eq_ignore_ascii_case("english")
                    || name.eq_ignore_ascii_case("en")
                {
                    en_branch = Some((start, j));
                }
                continue;
            }
            if c == 404 || c == 403 {
                break;
            }
            j += 1;
        }

        let template = en_branch.or_else(|| branches.first().copied());
        if let Some((start, end)) = template {
            let mut new_cmds: Vec<serde_json::Value> =
                list[start..end].iter().cloned().collect();
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
            for cmd in &mut new_cmds {
                let c = cmd.get("code").and_then(|x| x.as_u64()).unwrap_or(0);
                if c == 355 || c == 655 {
                    if let Some(params) = cmd.get_mut("parameters").and_then(|p| p.as_array_mut()) {
                        if let Some(s) = params.first_mut().and_then(|v| v.as_str()).map(|s| s.to_string()) {
                            let rewritten = rewrite_lang_script(&s, lang, new_index as i32);
                            params[0] = serde_json::Value::String(rewritten);
                        }
                    }
                }
            }
            // Insert before 404
            let mut insert_at = i + 1;
            while insert_at < list.len() {
                let c = list[insert_at].get("code").and_then(|x| x.as_u64()).unwrap_or(0);
                if c == 404 {
                    break;
                }
                insert_at += 1;
            }
            for (k, cmd) in new_cmds.into_iter().enumerate() {
                list.insert(insert_at + k, cmd);
            }
        }
        i += 1;
    }
    changed
}

fn rewrite_lang_script(script: &str, lang: &str, index: i32) -> String {
    let mut s = script.to_string();
    // IAVRA.MasterLocalization.I18N.language = "en";
    if s.contains("I18N.language") {
        s = regex_replace_lang_assign(&s, lang);
    }
    if s.contains("ConfigManager.lang") {
        s = s
            .lines()
            .map(|line| {
                if line.contains("ConfigManager.lang") {
                    // keep indentation
                    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                    format!("{indent}ConfigManager.lang = {index};")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !s.contains('\n') && s.contains("ConfigManager.lang") {
            s = format!("ConfigManager.lang = {index};");
        }
    }
    s
}

fn regex_replace_lang_assign(script: &str, lang: &str) -> String {
    // Simple state machine: language = "xx" or 'xx'
    let mut out = String::new();
    let bytes = script.as_bytes();
    let mut i = 0;
    let needle = "language";
    while i < bytes.len() {
        if script[i..].starts_with(needle) {
            out.push_str(needle);
            i += needle.len();
            // skip spaces and =
            while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                out.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'=' {
                out.push('=');
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_whitespace() {
                    out.push(bytes[i] as char);
                    i += 1;
                }
                if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                    let q = bytes[i] as char;
                    out.push(q);
                    i += 1;
                    while i < bytes.len() && bytes[i] as char != q {
                        i += 1;
                    }
                    out.push_str(lang);
                    if i < bytes.len() {
                        out.push(q);
                        i += 1;
                    }
                    continue;
                }
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
            fs::read_to_string(data.join("Map012.jsono")).unwrap().trim(),
        )
        .unwrap();
        let map_text = String::from_utf16_lossy(&units);
        assert!(map_text.contains("Español"), "{map_text}");
        assert!(map_text.contains("language = \\\"es\\\"") || map_text.contains("language = \"es\""));

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
