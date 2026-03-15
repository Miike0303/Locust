use crate::error::{LocustError, Result};
use crate::models::StringEntry;

use serde::{Deserialize, Serialize};

// ─── PO format ─────────────────────────────────────────────────────────────

pub fn export_po(entries: &[StringEntry], source_lang: &str, target_lang: &str) -> String {
    let mut lines = Vec::new();

    // Header
    lines.push("# Project Locust export".to_string());
    lines.push(format!("# Source: {}, Target: {}", source_lang, target_lang));
    lines.push(String::new());
    lines.push("msgid \"\"".to_string());
    lines.push("msgstr \"\"".to_string());
    lines.push(format!(
        "\"Content-Type: text/plain; charset=UTF-8\\n\""
    ));
    lines.push("\"Content-Transfer-Encoding: 8bit\\n\"".to_string());
    lines.push(format!("\"Language: {}\\n\"", target_lang));
    lines.push(String::new());

    // Entries
    for entry in entries {
        if let Some(ref ctx) = entry.context {
            lines.push(format!("#. {}", ctx));
        }
        lines.push(format!(
            "#: {}#{}",
            entry.file_path.display(),
            entry.id
        ));
        lines.push(format!("msgid \"{}\"", escape_po(&entry.source)));
        let translation = entry.translation.as_deref().unwrap_or("");
        lines.push(format!("msgstr \"{}\"", escape_po(translation)));
        lines.push(String::new());
    }

    lines.join("\n")
}

pub fn import_po(content: &str) -> Result<Vec<PoEntry>> {
    let mut entries = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_msgid: Option<String> = None;
    let mut current_msgstr: Option<String> = None;
    let mut reading = ReadingState::None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            // Flush current entry
            if let (Some(msgid), Some(msgstr)) = (current_msgid.take(), current_msgstr.take()) {
                if !msgid.is_empty() {
                    entries.push(PoEntry {
                        id: current_id.take(),
                        source: msgid,
                        translation: msgstr,
                    });
                }
            }
            current_id = None;
            reading = ReadingState::None;
            continue;
        }

        if trimmed.starts_with("#: ") {
            let reference = &trimmed[3..];
            // Extract id from reference (after last #)
            if let Some(hash_pos) = reference.rfind('#') {
                current_id = Some(reference[hash_pos + 1..].to_string());
            }
            continue;
        }

        if trimmed.starts_with("#") {
            continue;
        }

        if trimmed.starts_with("msgid ") {
            let val = extract_po_string(&trimmed[6..]);
            current_msgid = Some(unescape_po(&val));
            reading = ReadingState::Msgid;
            continue;
        }

        if trimmed.starts_with("msgstr ") {
            let val = extract_po_string(&trimmed[7..]);
            current_msgstr = Some(unescape_po(&val));
            reading = ReadingState::Msgstr;
            continue;
        }

        // Continuation line (quoted string)
        if trimmed.starts_with('"') {
            let val = extract_po_string(trimmed);
            let unescaped = unescape_po(&val);
            match reading {
                ReadingState::Msgid => {
                    if let Some(ref mut s) = current_msgid {
                        s.push_str(&unescaped);
                    }
                }
                ReadingState::Msgstr => {
                    if let Some(ref mut s) = current_msgstr {
                        s.push_str(&unescaped);
                    }
                }
                _ => {}
            }
        }
    }

    // Flush last entry
    if let (Some(msgid), Some(msgstr)) = (current_msgid, current_msgstr) {
        if !msgid.is_empty() {
            entries.push(PoEntry {
                id: current_id,
                source: msgid,
                translation: msgstr,
            });
        }
    }

    Ok(entries)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoEntry {
    pub id: Option<String>,
    pub source: String,
    pub translation: String,
}

enum ReadingState {
    None,
    Msgid,
    Msgstr,
}

fn escape_po(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn unescape_po(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn extract_po_string(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

// ─── XLIFF format ──────────────────────────────────────────────────────────

pub fn export_xliff(entries: &[StringEntry], source_lang: &str, target_lang: &str) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<xliff version=\"1.2\" xmlns=\"urn:oasis:names:tc:xliff:document:1.2\">\n");
    xml.push_str(&format!(
        "  <file source-language=\"{}\" target-language=\"{}\" datatype=\"plaintext\">\n",
        escape_xml(source_lang),
        escape_xml(target_lang)
    ));
    xml.push_str("    <body>\n");

    for entry in entries {
        let translation = entry.translation.as_deref().unwrap_or("");
        xml.push_str(&format!(
            "      <trans-unit id=\"{}\">\n",
            escape_xml(&entry.id)
        ));
        xml.push_str(&format!(
            "        <source>{}</source>\n",
            escape_xml(&entry.source)
        ));
        xml.push_str(&format!(
            "        <target>{}</target>\n",
            escape_xml(translation)
        ));
        xml.push_str("      </trans-unit>\n");
    }

    xml.push_str("    </body>\n");
    xml.push_str("  </file>\n");
    xml.push_str("</xliff>\n");
    xml
}

pub fn import_xliff(content: &str) -> Result<Vec<XliffUnit>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(content);
    let mut units = Vec::new();
    let mut current_id = String::new();
    let mut current_source = String::new();
    let mut current_target = String::new();
    let mut in_source = false;
    let mut in_target = false;
    let mut in_trans_unit = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                match e.name().as_ref() {
                    b"trans-unit" => {
                        in_trans_unit = true;
                        current_id = String::new();
                        current_source = String::new();
                        current_target = String::new();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"id" {
                                current_id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"source" if in_trans_unit => in_source = true,
                    b"target" if in_trans_unit => in_target = true,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_source {
                    current_source.push_str(&text);
                } else if in_target {
                    current_target.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"source" => in_source = false,
                    b"target" => in_target = false,
                    b"trans-unit" => {
                        in_trans_unit = false;
                        units.push(XliffUnit {
                            id: current_id.clone(),
                            source: current_source.clone(),
                            target: current_target.clone(),
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(LocustError::ParseError {
                    file: "xliff".to_string(),
                    message: format!("XLIFF parse error: {}", e),
                });
            }
            _ => {}
        }
    }

    Ok(units)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XliffUnit {
    pub id: String,
    pub source: String,
    pub target: String,
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_entries() -> Vec<StringEntry> {
        let mut e1 = StringEntry::new("e1", "Hello", PathBuf::from("test.json"));
        e1.translation = Some("Hola".to_string());
        e1.context = Some("greeting".to_string());

        let mut e2 = StringEntry::new("e2", "World", PathBuf::from("test.json"));
        e2.translation = Some("Mundo".to_string());

        let e3 = StringEntry::new("e3", "Untranslated", PathBuf::from("test.json"));

        vec![e1, e2, e3]
    }

    #[test]
    fn test_export_po_header() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        assert!(po.starts_with("# Project Locust export"));
        assert!(po.contains("\"Language: en\\n\""));
    }

    #[test]
    fn test_export_po_entries() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        let msgid_count = po.matches("msgid \"").count();
        // 1 header msgid + 3 entry msgids
        assert_eq!(msgid_count, 4);
    }

    #[test]
    fn test_export_po_empty_translation() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        // e3 has no translation
        assert!(po.contains("msgid \"Untranslated\"\nmsgstr \"\""));
    }

    #[test]
    fn test_import_po_roundtrip() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        let imported = import_po(&po).unwrap();
        assert_eq!(imported.len(), 3);
        assert_eq!(imported[0].source, "Hello");
        assert_eq!(imported[0].translation, "Hola");
        assert_eq!(imported[1].source, "World");
        assert_eq!(imported[1].translation, "Mundo");
        assert_eq!(imported[2].source, "Untranslated");
        assert_eq!(imported[2].translation, "");
    }

    #[test]
    fn test_export_xliff_structure() {
        let entries = make_entries();
        let xliff = export_xliff(&entries, "ja", "en");
        assert!(xliff.contains("<xliff version=\"1.2\""));
        assert!(xliff.contains("source-language=\"ja\""));
        assert!(xliff.contains("target-language=\"en\""));
        assert!(xliff.contains("<trans-unit id=\"e1\">"));
        assert!(xliff.contains("<source>Hello</source>"));
        assert!(xliff.contains("<target>Hola</target>"));
    }

    #[test]
    fn test_import_xliff_roundtrip() {
        let entries = make_entries();
        let xliff = export_xliff(&entries, "ja", "en");
        let imported = import_xliff(&xliff).unwrap();
        assert_eq!(imported.len(), 3);
        assert_eq!(imported[0].id, "e1");
        assert_eq!(imported[0].source, "Hello");
        assert_eq!(imported[0].target, "Hola");
        assert_eq!(imported[2].source, "Untranslated");
        assert_eq!(imported[2].target, "");
    }
}
