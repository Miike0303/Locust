use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for SugarCube/Twine HTML games.
/// SugarCube stores story passages inside `<tw-passagedata>` tags in a single HTML file.
pub struct SugarCubePlugin;

impl SugarCubePlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_html_file(path: &Path) -> Option<PathBuf> {
        if path.is_file() && is_html(path) {
            return Some(path.to_path_buf());
        }
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if is_html(&p) {
                        if let Ok(content) = std::fs::read_to_string(&p) {
                            if content.contains("tw-passagedata") || content.contains("SugarCube") {
                                return Some(p);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn extract_passages(content: &str, file_path: &Path) -> Vec<StringEntry> {
        let mut entries = Vec::new();

        let mut search_from = 0;
        while let Some(tag_start) = content[search_from..].find("<tw-passagedata") {
            let abs_start = search_from + tag_start;

            let tag_header_end = match content[abs_start..].find('>') {
                Some(pos) => abs_start + pos + 1,
                None => break,
            };

            let close_tag = "</tw-passagedata>";
            let tag_end = match content[tag_header_end..].find(close_tag) {
                Some(pos) => tag_header_end + pos,
                None => break,
            };

            let header = &content[abs_start..tag_header_end];
            let passage_name = extract_attr(header, "name").unwrap_or_default();
            let _pid = extract_attr(header, "pid").unwrap_or_default();
            let tags = extract_attr(header, "tags").unwrap_or_default();

            let raw_content = &content[tag_header_end..tag_end];
            let decoded = decode_html_entities(raw_content);

            // Skip system/widget/script passages
            if is_system_passage(&passage_name, &tags) || decoded.trim().is_empty() {
                search_from = tag_end + close_tag.len();
                continue;
            }

            let lines = extract_text_from_passage(&decoded);

            for (line_idx, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }

                // Protect SugarCube variables ($var, _var) with placeholders
                let (protected, var_map) = protect_variables(line);

                if protected.trim().is_empty() {
                    continue;
                }

                let id = format!("passage_{}#{}#{}", _pid, passage_name, line_idx);
                let mut entry = StringEntry::new(id, protected.as_str(), file_path.to_path_buf());
                entry.tags = vec!["dialogue".to_string()];
                entry.context = Some(passage_name.clone());

                // Store variable mapping in metadata for restoration during injection
                if !var_map.is_empty() {
                    let var_json: Vec<serde_json::Value> = var_map
                        .iter()
                        .map(|(placeholder, original)| {
                            serde_json::json!({"p": placeholder, "v": original})
                        })
                        .collect();
                    entry.metadata.insert(
                        "sugarcube_vars".to_string(),
                        serde_json::Value::Array(var_json),
                    );
                }

                entries.push(entry);
            }

            search_from = tag_end + close_tag.len();
        }

        entries
    }
}

/// Check if a passage is a system/code passage that should not be translated.
fn is_system_passage(name: &str, tags: &str) -> bool {
    // Skip known system passages
    let system_names = [
        "StoryInit", "StoryCaption", "StoryBanner", "StoryMenu",
        "StoryInterface", "StoryShare", "StoryAuthor", "StoryTitle",
        "StorySubtitle", "StoryDisplayTitle",
        "PassageHeader", "PassageFooter", "PassageReady", "PassageDone",
    ];
    if system_names.iter().any(|s| name.starts_with(s)) {
        return true;
    }

    // Skip passages tagged as widget, script, or stylesheet
    let skip_tags = ["widget", "script", "stylesheet", "init", "nobr-all"];
    let passage_tags: Vec<&str> = tags.split_whitespace().collect();
    if skip_tags.iter().any(|t| passage_tags.contains(t)) {
        return true;
    }

    false
}

fn is_html(path: &Path) -> bool {
    path.extension()
        .map_or(false, |e| e == "html" || e == "htm")
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
}

/// Protect SugarCube variables ($var, _var) by replacing them with numbered placeholders.
/// Returns the protected text and a list of (placeholder, original_variable) pairs.
fn protect_variables(text: &str) -> (String, Vec<(String, String)>) {
    let mut result = String::new();
    let mut vars = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Detect SugarCube permanent variable $varname
        if chars[i] == '$' && i + 1 < len && chars[i + 1].is_alphabetic() {
            let start = i;
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let var_name: String = chars[start..i].iter().collect();
            let placeholder = format!("{{{}}}", vars.len());
            vars.push((placeholder.clone(), var_name));
            result.push_str(&placeholder);
            continue;
        }

        // Detect SugarCube temporary variable _varname (at word boundary)
        if chars[i] == '_' && i + 1 < len && chars[i + 1].is_alphabetic() {
            // Only treat as variable if at start of text or after whitespace/punctuation
            let is_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
            if is_word_start {
                let start = i;
                i += 1;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let var_name: String = chars[start..i].iter().collect();
                let placeholder = format!("{{{}}}", vars.len());
                vars.push((placeholder.clone(), var_name));
                result.push_str(&placeholder);
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    (result, vars)
}

/// Restore variable placeholders in translated text back to original SugarCube variables.
fn restore_variables(text: &str, var_map: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (placeholder, original) in var_map {
        result = result.replace(placeholder, original);
    }
    result
}

/// Extract variable mapping from entry metadata.
fn get_var_map(entry: &StringEntry) -> Vec<(String, String)> {
    entry
        .metadata
        .get("sugarcube_vars")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let p = item.get("p")?.as_str()?.to_string();
                    let v = item.get("v")?.as_str()?.to_string();
                    Some((p, v))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Extract readable text from SugarCube passage content.
/// Strips macros (<<...>>), HTML tags, and SugarCube markup.
fn extract_text_from_passage(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut in_macro = false;
    let mut in_tag = false;

    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Detect macro start <<
        if i + 1 < len && chars[i] == '<' && chars[i + 1] == '<' {
            in_macro = true;
            i += 2;

            // Check if this is a <<script>> or <<widget>> block — skip until closing tag
            if i < len {
                let remaining: String = chars[i..std::cmp::min(i + 10, len)].iter().collect();
                if remaining.starts_with("script") || remaining.starts_with("widget") {
                    let close_tag = if remaining.starts_with("script") {
                        "<</script>>"
                    } else {
                        "<</widget>>"
                    };
                    let close_chars: Vec<char> = close_tag.chars().collect();
                    let close_len = close_chars.len();
                    while i + close_len <= len {
                        let window: String = chars[i..i + close_len].iter().collect();
                        if window == close_tag {
                            i += close_len;
                            break;
                        }
                        i += 1;
                    }
                    in_macro = false;
                    continue;
                }
            }

            // Skip until >>
            while i + 1 < len {
                if chars[i] == '>' && i > 0 && chars[i - 1] == '>' {
                    i += 1;
                    in_macro = false;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // HTML tags — skip entirely including attributes
        if chars[i] == '<' && !in_macro {
            in_tag = true;
            i += 1;
            continue;
        }
        if chars[i] == '>' && in_tag {
            in_tag = false;
            i += 1;
            continue;
        }

        if in_tag || in_macro {
            i += 1;
            continue;
        }

        // SugarCube link syntax: [[text|target]] or [[text->target]] or [[text]]
        if i + 1 < len && chars[i] == '[' && chars[i + 1] == '[' {
            i += 2;
            let mut link_text = String::new();
            let mut has_separator = false;
            while i < len {
                if i + 1 < len && chars[i] == ']' && chars[i + 1] == ']' {
                    i += 2;
                    break;
                }
                if chars[i] == '|' || (chars[i] == '-' && i + 1 < len && chars[i + 1] == '>') {
                    has_separator = true;
                    break;
                }
                link_text.push(chars[i]);
                i += 1;
            }
            // Skip to ]]
            if has_separator {
                while i < len {
                    if i + 1 < len && chars[i] == ']' && chars[i + 1] == ']' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            // Only include link display text (with separator), not bare passage names
            if has_separator && !link_text.trim().is_empty() {
                current_line.push_str(&link_text);
            }
            continue;
        }

        // Newlines end current text block
        if chars[i] == '\n' {
            let trimmed = current_line.trim().to_string();
            if !trimmed.is_empty() && is_extractable_line(&trimmed) {
                lines.push(trimmed);
            }
            current_line.clear();
            i += 1;
            continue;
        }

        current_line.push(chars[i]);
        i += 1;
    }

    // Flush last line
    let trimmed = current_line.trim().to_string();
    if !trimmed.is_empty() && is_extractable_line(&trimmed) {
        lines.push(trimmed);
    }

    // Filter out CSS, JS, and code-like lines
    lines.into_iter().filter(|line| is_translatable_text(line)).collect()
}

/// Check if a line is extractable (not just a variable, path, or code fragment).
fn is_extractable_line(line: &str) -> bool {
    let s = line.trim();

    // Skip lines starting with / (comments) or $ (pure variable assignments)
    if s.starts_with('/') || s.starts_with('$') {
        return false;
    }

    // Skip lines that are just a bare SugarCube variable (_var or $var)
    if (s.starts_with('_') || s.starts_with('$')) && s[1..].chars().all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }

    // Skip lines that look like file paths
    if (s.contains('/') || s.contains('\\')) && !s.contains(' ') {
        return false;
    }

    // Skip lines that are just numbers or punctuation
    if s.chars().all(|c| !c.is_alphabetic()) {
        return false;
    }

    true
}

/// Returns false for lines that look like CSS, JavaScript, or code.
fn is_translatable_text(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }

    // CSS properties
    if s.contains(':') && (
        s.contains("px") || s.contains("em") || s.contains("rem") || s.contains("vh") || s.contains("vw") ||
        s.contains("rgb") || (s.contains("#") && s.len() < 50) ||
        s.contains("var(--") || s.contains("solid") || s.contains("none;") ||
        s.contains("flex") || s.contains("grid") || s.contains("block") ||
        s.contains("absolute") || s.contains("relative") || s.contains("fixed")
    ) {
        return false;
    }

    if s.ends_with(';') && s.contains(':') {
        return false;
    }
    if s.starts_with('.') && s.contains('{') {
        return false;
    }
    if s.contains("background") || s.contains("font-size") || s.contains("margin") ||
       s.contains("padding") || s.contains("border") || s.contains("display:") ||
       s.contains("position:") || s.contains("color:") || s.contains("width:") ||
       s.contains("height:") || s.contains("text-align") || s.contains("box-shadow") ||
       s.contains("opacity") || s.contains("z-index") || s.contains("overflow") ||
       s.contains("transform") || s.contains("transition") || s.contains("cursor:") {
        return false;
    }

    // JavaScript patterns
    if s.starts_with("var ") || s.starts_with("let ") || s.starts_with("const ") ||
       s.starts_with("function") || s.starts_with("return ") || s.starts_with("if (") ||
       s.starts_with("else") || s.starts_with("for (") || s.starts_with("while (") ||
       s.contains("document.") || s.contains("window.") || s.contains("console.") ||
       s.contains("addEventListener") || s.contains("querySelector") || s.contains("setTimeout") ||
       s.contains("=>") || s.contains("===") || s.contains("!==") {
        return false;
    }

    // HTML/CSS class/id references
    if s.starts_with('#') && !s.contains(' ') {
        return false;
    }

    // Only punctuation/symbols, no real words
    let alpha_count = s.chars().filter(|c| c.is_alphabetic()).count();
    let total = s.chars().count();
    if total > 0 && (alpha_count as f64 / total as f64) < 0.3 {
        return false;
    }

    true
}

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn id(&self) -> &str {
        "sugarcube"
    }

    fn name(&self) -> &str {
        "SugarCube/Twine HTML"
    }

    fn description(&self) -> &str {
        "SugarCube/Twine interactive fiction HTML games"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::ComingSoon
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".html", ".htm"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        Self::find_html_file(path).is_some()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let html_file = Self::find_html_file(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "no SugarCube HTML file found".to_string(),
        })?;

        let content = std::fs::read_to_string(&html_file)?;
        Ok(Self::extract_passages(&content, &html_file))
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let html_file = Self::find_html_file(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "no SugarCube HTML file found".to_string(),
        })?;

        let content = std::fs::read_to_string(&html_file)?;
        let mut written = 0;
        let mut skipped = 0;

        // Collect all passage names — these must NEVER be translated
        let mut passage_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        {
            let mut scan = 0;
            while let Some(pos) = content[scan..].find("<tw-passagedata") {
                let abs = scan + pos;
                if let Some(header_end) = content[abs..].find('>') {
                    let header = &content[abs..abs + header_end + 1];
                    if let Some(name) = extract_attr(header, "name") {
                        passage_names.insert(name);
                    }
                    scan = abs + header_end + 1;
                } else {
                    break;
                }
            }
        }

        // Only replace text WITHIN <tw-passagedata> tags
        let mut result = String::with_capacity(content.len());
        let mut search_from = 0;

        while let Some(tag_start) = content[search_from..].find("<tw-passagedata") {
            let abs_start = search_from + tag_start;
            result.push_str(&content[search_from..abs_start]);

            let tag_header_end = match content[abs_start..].find('>') {
                Some(pos) => abs_start + pos + 1,
                None => {
                    result.push_str(&content[abs_start..]);
                    search_from = content.len();
                    break;
                }
            };

            let close_tag = "</tw-passagedata>";
            let tag_end = match content[tag_header_end..].find(close_tag) {
                Some(pos) => tag_header_end + pos,
                None => {
                    result.push_str(&content[abs_start..]);
                    search_from = content.len();
                    break;
                }
            };

            // Copy the opening tag header
            result.push_str(&content[abs_start..tag_header_end]);

            // Get passage content and do replacements only within it
            let mut passage_content = content[tag_header_end..tag_end].to_string();
            for entry in entries {
                if let Some(ref translation) = entry.translation {
                    // Restore variables in both source and translation
                    let var_map = get_var_map(entry);
                    let original_source = restore_variables(&entry.source, &var_map);
                    let final_translation = restore_variables(translation, &var_map);

                    // Skip if the source text matches a passage name
                    if passage_names.contains(&original_source) {
                        continue;
                    }

                    let encoded_source = encode_html_entities(&original_source);
                    let encoded_translation = encode_html_entities(&final_translation);

                    // Safe replacement: skip macros, links, and HTML tags
                    let (new_content, did_replace) = replace_safe(
                        &passage_content,
                        &encoded_source,
                        &encoded_translation,
                    );
                    if did_replace {
                        passage_content = new_content;
                        written += 1;
                    }
                }
            }

            result.push_str(&passage_content);
            result.push_str(close_tag);
            search_from = tag_end + close_tag.len();
        }

        // Copy remainder after last passage
        if search_from < content.len() {
            result.push_str(&content[search_from..]);
        }

        for entry in entries {
            if entry.translation.is_none() {
                skipped += 1;
            }
        }

        std::fs::write(&html_file, &result)?;

        Ok(InjectionReport {
            files_modified: if written > 0 { 1 } else { 0 },
            strings_written: written,
            strings_skipped: skipped,
            warnings: Vec::new(),
        })
    }
}

fn encode_html_entities(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Safe replacement in SugarCube passage content.
/// Avoids replacing inside:
/// - HTML-encoded tags (&lt;...&gt;)
/// - SugarCube macros (&lt;&lt;...&gt;&gt;)
/// - SugarCube link targets ([[...|TARGET]] or [[...->TARGET]] or [[TARGET]])
/// - SugarCube variable expressions
fn replace_safe(passage: &str, source: &str, replacement: &str) -> (String, bool) {
    if source.is_empty() {
        return (passage.to_string(), false);
    }

    let mut pos = 0;
    while let Some(match_pos) = passage[pos..].find(source) {
        let abs_pos = pos + match_pos;
        let match_end = abs_pos + source.len();

        // Check if inside an HTML-encoded tag
        if is_inside_unsafe_context(passage, abs_pos, match_end) {
            pos = abs_pos + 1;
            continue;
        }

        let mut result = String::with_capacity(passage.len());
        result.push_str(&passage[..abs_pos]);
        result.push_str(replacement);
        result.push_str(&passage[match_end..]);
        return (result, true);
    }

    (passage.to_string(), false)
}

/// Check if a position falls inside a context that should not be modified:
/// encoded HTML tags, SugarCube macros, link targets, or code blocks.
fn is_inside_unsafe_context(s: &str, pos: usize, end: usize) -> bool {
    let before = &s[..pos];

    // Inside HTML-encoded tag: &lt;...&gt;
    let last_lt = before.rfind("&lt;");
    let last_gt = before.rfind("&gt;");
    if let Some(lt_pos) = last_lt {
        match last_gt {
            Some(gt_pos) if lt_pos > gt_pos => return true,
            None => return true,
            _ => {}
        }
    }

    // Inside SugarCube macro: &lt;&lt;...&gt;&gt;
    let last_macro_open = before.rfind("&lt;&lt;");
    let last_macro_close = before.rfind("&gt;&gt;");
    if let Some(mo) = last_macro_open {
        match last_macro_close {
            Some(mc) if mo > mc => return true,
            None => return true,
            _ => {}
        }
    }

    // Inside SugarCube link: [[...]]
    // Check if we're between [[ and ]]
    let last_link_open = before.rfind("[[");
    let last_link_close = before.rfind("]]");
    if let Some(lo) = last_link_open {
        let inside_link = match last_link_close {
            Some(lc) => lo > lc,
            None => true,
        };
        if inside_link {
            // We're inside [[ ... ]]. Only allow replacing the DISPLAY text
            // (before | or ->), never the target (after | or ->)
            let link_content = &s[lo + 2..];
            let link_end = link_content.find("]]").unwrap_or(link_content.len());
            let link_inner = &link_content[..link_end];

            // Find separator position relative to link start
            let sep_pipe = link_inner.find('|');
            let sep_arrow = link_inner.find("-&gt;");

            let rel_pos = pos - (lo + 2);

            if let Some(sp) = sep_pipe {
                // [[display|target]] — only translate display part (before |)
                if rel_pos >= sp {
                    return true; // in target part
                }
            } else if let Some(sa) = sep_arrow {
                // [[display->target]] — only translate display part (before ->)
                if rel_pos >= sa {
                    return true; // in target part
                }
            } else {
                // [[bare passage name]] — do NOT translate (it's a passage reference)
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_sc_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_fixture(dir: &Path) -> PathBuf {
        let html = dir.join("game.html");
        fs::write(&html, r#"<!DOCTYPE html>
<html>
<head><meta name="application-name" content="SugarCube" /></head>
<body>
<tw-storydata name="Test">
<tw-passagedata pid="1" name="Start" tags="">Hello, welcome to the game!
This is the second line.
&lt;&lt;set $name = "player"&gt;&gt;
[[Continue|next]]</tw-passagedata>
<tw-passagedata pid="2" name="next" tags="">You chose to continue.
&lt;&lt;if $health &gt; 0&gt;&gt;You are alive.&lt;&lt;/if&gt;&gt;
The adventure awaits!</tw-passagedata>
</tw-storydata>
</body></html>"#).unwrap();
        html
    }

    #[test]
    fn test_detect_sugarcube() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_sugarcube() {
        let dir = tempdir();
        fs::write(dir.join("index.html"), "<html><body>normal</body></html>").unwrap();
        let plugin = SugarCubePlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_passages() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(entries.len() >= 4, "got {} entries: {:?}", entries.len(), entries.iter().map(|e| &e.source).collect::<Vec<_>>());

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.iter().any(|s| s.contains("welcome")));
        assert!(sources.iter().any(|s| s.contains("adventure")));
    }

    #[test]
    fn test_extract_strips_macros() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        for e in &entries {
            assert!(!e.source.contains("<<"), "source should not contain macros: {}", e.source);
        }
    }

    #[test]
    fn test_inject_replace() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source.contains("welcome") {
                entry.translation = Some("Bienvenido al juego!".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1);
    }

    #[test]
    fn test_context_is_passage_name() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(entries.iter().any(|e| e.context == Some("Start".to_string())));
        assert!(entries.iter().any(|e| e.context == Some("next".to_string())));
    }

    #[test]
    fn test_protect_variables() {
        let (protected, vars) = protect_variables("Hey $name, how are you?");
        assert_eq!(protected, "Hey {0}, how are you?");
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].1, "$name");

        let restored = restore_variables("Hola {0}, ¿cómo estás?", &vars);
        assert_eq!(restored, "Hola $name, ¿cómo estás?");
    }

    #[test]
    fn test_protect_temp_variables() {
        let (protected, vars) = protect_variables("Value is _contents here");
        assert!(protected.contains("{0}"));
        assert_eq!(vars[0].1, "_contents");
    }

    #[test]
    fn test_skip_system_passages() {
        assert!(is_system_passage("PassageReady", ""));
        assert!(is_system_passage("PassageDone", ""));
        assert!(is_system_passage("StoryInit", ""));
        assert!(is_system_passage("SomeWidget", "widget"));
        assert!(!is_system_passage("Introduction", ""));
    }

    #[test]
    fn test_tag_safe_replacement() {
        let passage = "Clothes are nice &lt;img src=&quot;Images/Clothes/1.webp&quot;&gt;";
        let (result, replaced) = replace_safe(passage, "Clothes", "Ropa");
        assert!(replaced);
        // Should replace the visible text "Clothes" but NOT the one in the img tag
        assert!(result.starts_with("Ropa"));
        assert!(result.contains("Images/Clothes/1.webp"));
    }

    #[test]
    fn test_no_replace_in_link_target() {
        // [[display text|PassageName]] — should NOT translate PassageName
        let passage = "[[Open laptop|Open laptop]]";
        let (result, replaced) = replace_safe(passage, "Open laptop", "Abrir portátil");
        assert!(replaced);
        // Display text should be translated, target should NOT
        assert!(result.contains("Abrir portátil|Open laptop"));
    }

    #[test]
    fn test_no_replace_bare_link() {
        // [[PassageName]] — bare link, should NOT translate
        let passage = "some text [[Open laptop]] more text";
        let (result, _) = replace_safe(passage, "Open laptop", "Abrir portátil");
        // The text inside bare link should NOT be translated
        assert!(result.contains("[[Open laptop]]"));
    }

    #[test]
    fn test_no_replace_in_macro() {
        let passage = "&lt;&lt;set $name to &quot;John&quot;&gt;&gt; Hello John!";
        let (result, replaced) = replace_safe(passage, "John", "Juan");
        assert!(replaced);
        // Should replace "John" in text but NOT inside the macro
        assert!(result.contains("Hello Juan!"));
        assert!(result.contains("&lt;&lt;set $name to &quot;John&quot;&gt;&gt;"));
    }

    #[test]
    fn test_skip_widget_passages() {
        let html = r#"<tw-passagedata pid="1" name="widgets" tags="widget">&lt;&lt;widget "test"&gt;&gt;_contents&lt;&lt;/widget&gt;&gt;</tw-passagedata>"#;
        let entries = SugarCubePlugin::extract_passages(html, Path::new("test.html"));
        assert!(entries.is_empty(), "widget passages should be skipped");
    }

    #[test]
    fn test_bare_variable_filtered() {
        assert!(!is_extractable_line("_contents"));
        assert!(!is_extractable_line("$name"));
        assert!(!is_extractable_line("Images/Clothes/1.webp"));
        assert!(is_extractable_line("Hello world"));
    }
}
