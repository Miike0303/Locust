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
                        // Check if it's a SugarCube file
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

        // Find all <tw-passagedata> tags
        let mut search_from = 0;
        while let Some(tag_start) = content[search_from..].find("<tw-passagedata") {
            let abs_start = search_from + tag_start;

            // Find the closing >
            let tag_header_end = match content[abs_start..].find('>') {
                Some(pos) => abs_start + pos + 1,
                None => break,
            };

            // Find closing tag
            let close_tag = "</tw-passagedata>";
            let tag_end = match content[tag_header_end..].find(close_tag) {
                Some(pos) => tag_header_end + pos,
                None => break,
            };

            // Extract passage name from pid and name attributes
            let header = &content[abs_start..tag_header_end];
            let passage_name = extract_attr(header, "name").unwrap_or_default();
            let pid = extract_attr(header, "pid").unwrap_or_default();

            // Get passage content (HTML-encoded)
            let raw_content = &content[tag_header_end..tag_end];
            let decoded = decode_html_entities(raw_content);

            // Skip system/widget passages
            if passage_name.starts_with("StoryInit")
                || passage_name.starts_with("StoryCaption")
                || passage_name.starts_with("PassageHeader")
                || passage_name.starts_with("PassageFooter")
                || decoded.trim().is_empty()
            {
                search_from = tag_end + close_tag.len();
                continue;
            }

            // Extract text lines from passage content
            // SugarCube syntax: text between macros like <<if>>, <<set>>, etc.
            let lines = extract_text_from_passage(&decoded);

            for (line_idx, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let id = format!("passage_{}#{}#{}", pid, passage_name, line_idx);
                let mut entry = StringEntry::new(id, line.as_str(), file_path.to_path_buf());
                entry.tags = vec!["dialogue".to_string()];
                entry.context = Some(passage_name.clone());
                entries.push(entry);
            }

            search_from = tag_end + close_tag.len();
        }

        entries
    }
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

/// Extract readable text from SugarCube passage content.
/// Strips macros (<<...>>), HTML tags, and SugarCube markup.
fn extract_text_from_passage(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut in_macro = false;
    let mut in_tag = false;
    let mut macro_depth = 0;

    let chars: Vec<char> = content.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Detect macro start <<
        if i + 1 < len && chars[i] == '<' && chars[i + 1] == '<' {
            in_macro = true;
            macro_depth += 1;
            i += 2;

            // Check if this is a <<script>> block — skip everything until <</script>>
            let remaining: String = chars[i..std::cmp::min(i + 8, len)].iter().collect();
            if remaining.starts_with("script") {
                // Skip until <</script>>
                while i + 1 < len {
                    if chars[i] == '<' && i + 10 < len {
                        let close: String = chars[i..i + 11].iter().collect();
                        if close == "<</script>>" {
                            i += 11;
                            break;
                        }
                    }
                    i += 1;
                }
                in_macro = false;
                continue;
            }

            // Skip until >>
            while i + 1 < len {
                if chars[i] == '>' && chars[i - 1] == '>' {
                    i += 1;
                    in_macro = false;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // HTML tags
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

        // SugarCube link syntax: [[text|target]] or [[text]]
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
                    // Everything after | or -> is the target, text is before
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
            // Only include link text if it has a separator (|/->)
            // [[text]] links are passage names — translating them breaks navigation
            if has_separator && !link_text.trim().is_empty() {
                current_line.push_str(&link_text);
            }
            continue;
        }

        // Newlines end current text block
        if chars[i] == '\n' {
            let trimmed = current_line.trim().to_string();
            if !trimmed.is_empty() && !trimmed.starts_with('/') && !trimmed.starts_with('$') {
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
    if !trimmed.is_empty() && !trimmed.starts_with('/') && !trimmed.starts_with('$') {
        lines.push(trimmed);
    }

    // Filter out CSS, JS, and code-like lines
    lines.into_iter().filter(|line| is_translatable_text(line)).collect()
}

/// Returns false for lines that look like CSS, JavaScript, or code.
fn is_translatable_text(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }

    // CSS properties (contain : followed by values with units, colors, etc.)
    if s.contains(':') && (
        s.contains("px") || s.contains("em") || s.contains("rem") || s.contains("vh") || s.contains("vw") ||
        s.contains("rgb") || s.contains("#") && s.len() < 50 ||
        s.contains("var(--") || s.contains("solid") || s.contains("none;") ||
        s.contains("flex") || s.contains("grid") || s.contains("block") ||
        s.contains("absolute") || s.contains("relative") || s.contains("fixed")
    ) {
        return false;
    }

    // CSS-like patterns
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

        // Only replace text WITHIN <tw-passagedata> tags, not in JS/CSS/HTML structure
        let mut result = String::with_capacity(content.len());
        let mut search_from = 0;

        while let Some(tag_start) = content[search_from..].find("<tw-passagedata") {
            let abs_start = search_from + tag_start;

            // Copy everything before this passage tag unchanged
            result.push_str(&content[search_from..abs_start]);

            // Find the closing > of the opening tag
            let tag_header_end = match content[abs_start..].find('>') {
                Some(pos) => abs_start + pos + 1,
                None => {
                    result.push_str(&content[abs_start..]);
                    search_from = content.len();
                    break;
                }
            };

            // Find the closing </tw-passagedata>
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
                    let encoded_source = encode_html_entities(&entry.source);
                    let encoded_translation = encode_html_entities(translation);
                    if passage_content.contains(&encoded_source) {
                        passage_content = passage_content.replacen(&encoded_source, &encoded_translation, 1);
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

        // Count skipped
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
        // No entry should contain <<set or <<if
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
}
