use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for generic HTML-based games (non-SugarCube).
/// Handles Twine/Harlowe, plain HTML adventure games, and other HTML game formats.
pub struct HtmlGamePlugin;

impl HtmlGamePlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_html_files(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if path.is_file() && is_html(path) {
            files.push(path.to_path_buf());
        } else if path.is_dir() {
            Self::scan_dir(path, &mut files, 0);
        }
        files
    }

    fn scan_dir(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
        if depth > 3 {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && is_html(&p) {
                    files.push(p);
                } else if p.is_dir() && !is_skip_dir(&p) {
                    Self::scan_dir(&p, files, depth + 1);
                }
            }
        }
    }

    fn is_sugarcube(content: &str) -> bool {
        content.contains("tw-passagedata") || content.contains("SugarCube")
    }

    fn extract_from_html(content: &str, file_path: &Path) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut pos = 0;
        let bytes = content.as_bytes();
        let len = bytes.len();

        while pos < len {
            if bytes[pos] == b'<' {
                let tag_start = pos;
                if let Some(gt) = content[pos..].find('>') {
                    let tag_header = &content[pos..pos + gt + 1];
                    let tag_name = extract_tag_name(tag_header);

                    // Skip script, style, head, svg, noscript
                    if is_skip_tag(&tag_name) {
                        if let Some(close) = find_close_tag(content, pos + gt + 1, &tag_name) {
                            pos = close;
                            continue;
                        }
                    }

                    // Extract translatable attributes
                    for attr in &["alt", "title", "placeholder", "aria-label", "label"] {
                        if let Some(val) = extract_attribute(tag_header, attr) {
                            let val = val.trim().to_string();
                            if is_translatable_text(&val) {
                                let id = format!(
                                    "{}#attr:{}:{}",
                                    file_path.display(),
                                    attr,
                                    entries.len()
                                );
                                let mut entry =
                                    StringEntry::new(id, val, file_path.to_path_buf());
                                entry.context = Some(format!("HTML attribute: {}", attr));
                                entry.tags = vec!["html-attr".to_string()];
                                entries.push(entry);
                            }
                        }
                    }

                    // For content-bearing tags, extract inner text
                    if is_text_tag(&tag_name) && !tag_header.ends_with("/>") {
                        let inner_start = pos + gt + 1;
                        if let Some(close) = find_close_tag(content, inner_start, &tag_name) {
                            let close_tag_str = format!("</{}", tag_name);
                            if let Some(close_pos) =
                                content[inner_start..close].find(&close_tag_str)
                            {
                                let inner = &content[inner_start..inner_start + close_pos];
                                let text = strip_inner_tags(inner).trim().to_string();
                                if is_translatable_text(&text) && text.len() >= 2 {
                                    let id = format!(
                                        "{}#text:{}:{}",
                                        file_path.display(),
                                        tag_name,
                                        entries.len()
                                    );
                                    let mut entry =
                                        StringEntry::new(id, text, file_path.to_path_buf());
                                    entry.context = Some(format!("<{}>", tag_name));
                                    entry.tags = vec!["html-text".to_string()];
                                    entry.metadata.insert(
                                        "raw_inner".to_string(),
                                        serde_json::Value::String(inner.to_string()),
                                    );
                                    entries.push(entry);
                                }
                            }
                            pos = close;
                            continue;
                        }
                    }

                    pos = tag_start + gt + 1;
                } else {
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }

        entries
    }
}

impl Default for HtmlGamePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for HtmlGamePlugin {
    fn id(&self) -> &str {
        "html-game"
    }

    fn name(&self) -> &str {
        "HTML Game (Generic)"
    }

    fn description(&self) -> &str {
        "Generic HTML-based games: Twine/Harlowe, HTML adventure games, interactive fiction"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".html", ".htm"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return false;
        }
        for f in &files {
            if let Ok(content) = std::fs::read_to_string(f) {
                if Self::is_sugarcube(&content) {
                    return false;
                }
            }
        }
        for f in &files {
            if let Ok(content) = std::fs::read_to_string(f) {
                let entries = Self::extract_from_html(&content, f);
                if !entries.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.to_string_lossy().to_string(),
                message: "No HTML files found".into(),
            });
        }

        let mut all_entries = Vec::new();
        for file in &files {
            if let Ok(content) = std::fs::read_to_string(file) {
                if Self::is_sugarcube(&content) {
                    continue;
                }
                let entries = Self::extract_from_html(&content, file);
                all_entries.extend(entries);
            }
        }

        Ok(all_entries)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.to_string_lossy().to_string(),
                message: "No HTML files found".into(),
            });
        }

        let mut translations: HashMap<String, String> = HashMap::new();
        for entry in entries {
            if let Some(ref t) = entry.translation {
                if !t.is_empty() {
                    translations.insert(entry.source.clone(), t.clone());
                }
            }
        }

        let mut report = InjectionReport {
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
        };

        for file in &files {
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if Self::is_sugarcube(&content) {
                continue;
            }

            let mut modified = content.clone();
            let mut file_changed = false;

            for (source, translation) in &translations {
                if modified.contains(source.as_str()) {
                    let safe_translation = html_encode_text(translation);
                    modified = modified.replace(source.as_str(), &safe_translation);
                    report.strings_written += 1;
                    file_changed = true;
                }
            }

            if file_changed {
                std::fs::write(file, &modified)?;
                report.files_modified += 1;
            }
        }

        report.strings_skipped = entries.len().saturating_sub(report.strings_written);
        Ok(report)
    }
}

fn is_html(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("html" | "htm")
    )
}

fn is_skip_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(
        name,
        "node_modules" | ".git" | "dist" | "build" | "__pycache__" | ".svn"
    )
}

fn extract_tag_name(tag: &str) -> String {
    let s = tag.trim_start_matches('<').trim_start_matches('/');
    let end = s
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(s.len());
    s[..end].to_lowercase()
}

fn is_skip_tag(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "svg" | "noscript" | "template"
    )
}

fn is_text_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "span" | "a" | "button"
            | "label" | "li" | "td" | "th" | "option" | "caption" | "figcaption" | "blockquote"
            | "cite" | "em" | "strong" | "b" | "i" | "small" | "title" | "legend" | "summary"
            | "dt" | "dd"
    )
}

fn find_close_tag(content: &str, from: usize, tag: &str) -> Option<usize> {
    let close = format!("</{}>", tag);
    let close_upper = format!("</{}>", tag.to_uppercase());
    if let Some(pos) = content[from..].find(&close) {
        return Some(from + pos + close.len());
    }
    if let Some(pos) = content[from..].find(&close_upper) {
        return Some(from + pos + close_upper.len());
    }
    None
}

fn extract_attribute(tag: &str, attr: &str) -> Option<String> {
    let patterns = [
        format!("{}=\"", attr),
        format!("{}='", attr),
        format!("{}=\"", attr.to_uppercase()),
    ];
    for pat in &patterns {
        if let Some(start) = tag.find(pat.as_str()) {
            let val_start = start + pat.len();
            let quote = tag.as_bytes()[val_start - 1] as char;
            if let Some(end) = tag[val_start..].find(quote) {
                return Some(tag[val_start..val_start + end].to_string());
            }
        }
    }
    None
}

fn strip_inner_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn is_translatable_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() < 2 {
        return false;
    }
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("//")
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.contains("://")
    {
        return false;
    }
    if trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '-')
    {
        return false;
    }
    if trimmed.starts_with('#')
        && trimmed.len() <= 7
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return false;
    }
    let special_ratio = trimmed
        .chars()
        .filter(|c| matches!(c, '{' | '}' | ';' | '=' | '(' | ')' | '[' | ']'))
        .count() as f64
        / trimmed.len() as f64;
    if special_ratio > 0.15 {
        return false;
    }
    trimmed.chars().any(|c| c.is_alphabetic())
}

fn html_encode_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn create_html_game(dir: &Path) -> PathBuf {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Adventure Game</title></head>
<body>
<script>var x = 1;</script>
<style>.red { color: red; }</style>
<div id="intro">
  <h1>Welcome to the Adventure</h1>
  <p>You find yourself in a dark forest.</p>
  <p>The trees are tall and menacing.</p>
</div>
<div id="choices">
  <button onclick="go(1)">Go north</button>
  <button onclick="go(2)">Go south</button>
</div>
<img src="forest.png" alt="A dark forest path" />
<input placeholder="Enter your name" />
</body>
</html>"#;
        let file = dir.join("game.html");
        fs::write(&file, html).unwrap();
        file
    }

    #[test]
    fn test_detect_html_game() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        assert!(plugin.detect(dir.path()));
    }

    #[test]
    fn test_detect_rejects_sugarcube() {
        let dir = tempdir().unwrap();
        let html =
            r#"<html><body><tw-passagedata name="Start">Hello</tw-passagedata></body></html>"#;
        fs::write(dir.path().join("game.html"), html).unwrap();
        let plugin = HtmlGamePlugin::new();
        assert!(!plugin.detect(dir.path()));
    }

    #[test]
    fn test_extract_text_elements() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Welcome to the Adventure"));
        assert!(sources.contains(&"You find yourself in a dark forest."));
        assert!(sources.contains(&"The trees are tall and menacing."));
        assert!(sources.contains(&"Go north"));
        assert!(sources.contains(&"Go south"));
        assert!(!sources.iter().any(|s| s.contains("var x")));
        assert!(!sources.iter().any(|s| s.contains("color: red")));
    }

    #[test]
    fn test_extract_attributes() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"A dark forest path"));
        assert!(sources.contains(&"Enter your name"));
    }

    #[test]
    fn test_extract_title() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Adventure Game"));
    }

    #[test]
    fn test_inject_replaces_text() {
        let dir = tempdir().unwrap();
        let file = create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();

        for entry in &mut entries {
            match entry.source.as_str() {
                "Go north" => entry.translation = Some("Ir al norte".to_string()),
                "Go south" => entry.translation = Some("Ir al sur".to_string()),
                "You find yourself in a dark forest." => {
                    entry.translation = Some("Te encuentras en un bosque oscuro.".to_string())
                }
                _ => {}
            }
        }

        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert!(report.files_modified >= 1);
        assert!(report.strings_written >= 3);

        let content = fs::read_to_string(&file).unwrap();
        assert!(content.contains("Ir al norte"));
        assert!(content.contains("Ir al sur"));
        assert!(content.contains("Te encuentras en un bosque oscuro."));
    }

    #[test]
    fn test_skips_code_like_text() {
        assert!(!is_translatable_text("var x = 1;"));
        assert!(!is_translatable_text("#ff0000"));
        assert!(!is_translatable_text("https://example.com"));
        assert!(!is_translatable_text("123.456"));
        assert!(!is_translatable_text(""));
        assert!(is_translatable_text("Hello world"));
        assert!(is_translatable_text("Go north"));
    }

    #[test]
    fn test_strip_inner_tags() {
        assert_eq!(strip_inner_tags("Hello <b>world</b>!"), "Hello world!");
        assert_eq!(strip_inner_tags("Plain text"), "Plain text");
        assert_eq!(strip_inner_tags("&amp; &lt; &gt;"), "& < >");
    }

    #[test]
    fn test_plugin_metadata() {
        let plugin = HtmlGamePlugin::new();
        assert_eq!(plugin.id(), "html-game");
        assert_eq!(plugin.name(), "HTML Game (Generic)");
        assert_eq!(plugin.supported_extensions(), &[".html", ".htm"]);
        assert_eq!(plugin.supported_modes(), vec![OutputMode::Replace]);
    }

    #[test]
    fn test_multi_file_game() {
        let dir = tempdir().unwrap();
        let html1 = r#"<html><body><p>Page one content</p></body></html>"#;
        let html2 = r#"<html><body><p>Page two content</p></body></html>"#;
        fs::write(dir.path().join("page1.html"), html1).unwrap();
        fs::write(dir.path().join("page2.html"), html2).unwrap();

        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();
        assert!(entries.len() >= 2);

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Page one content"));
        assert!(sources.contains(&"Page two content"));
    }
}
