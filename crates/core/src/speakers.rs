//! Conservative row identities shared by speaker APIs and translation context.
use crate::models::StringEntry;
use serde::{Deserialize, Serialize};

pub const NOTES_METADATA_KEY: &str = "locust.speakers.notes.v1";
pub type SpeakerNotes = std::collections::BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerIdentity {
    pub id: String,
    pub name: String,
}

fn identity(namespace: &str, name: &str) -> Option<SpeakerIdentity> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(SpeakerIdentity {
        id: serde_json::json!([namespace, name]).to_string(),
        name: name.into(),
    })
}

/// Explicit fields are aliases, not heuristics. Conflicting or malformed fields
/// fail closed. Tags naming a name-slot and arbitrary context are not speakers.
pub fn resolve_speaker(entry: &StringEntry, project_format: &str) -> Option<SpeakerIdentity> {
    let mut explicit = None;
    for key in ["speaker", "speaker_name", "character"] {
        if let Some(value) = entry.metadata.get(key) {
            let candidate = identity("explicit", value.as_str()?)?;
            if explicit
                .as_ref()
                .is_some_and(|previous| previous != &candidate)
            {
                return None;
            }
            explicit = Some(candidate);
        }
    }
    if explicit.is_some() {
        return explicit;
    }
    // Row provenance overrides project format, including malformed values.
    let format = entry.metadata.get("format_id").map_or_else(
        || {
            if !project_format.is_empty() {
                return Some(project_format);
            }
            // Translation runs have no project-format field in the database.
            // Accept only native extractor locators when that hint is absent.
            if entry.metadata.contains_key("godot_dialogic") {
                Some("godot-dialogic")
            } else if entry.id.rsplit_once('#').is_some_and(|(file, line)| {
                entry.file_path.file_name().and_then(|s| s.to_str()) == Some(file)
                    && !line.is_empty()
                    && line.bytes().all(|b| b.is_ascii_digit())
            }) {
                Some("renpy") // Still requires .rpy + dialogue + identifier below.
            } else {
                None
            }
        },
        |v| v.as_str(),
    )?;
    match format {
        "renpy"
            if entry.tags.iter().any(|tag| tag == "dialogue")
                && entry.file_path.extension().is_some_and(|ext| ext == "rpy") =>
        {
            let name = entry.context.as_deref()?;
            // The say extractor stores one identifier, never layout/descriptive text.
            let mut chars = name.chars();
            if !chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
                || !chars.all(|c| c == '_' || c.is_alphanumeric())
            {
                return None;
            }
            identity("renpy", name)
        }
        "godot-dialogic" => {
            let locator = entry.metadata.get("godot_dialogic")?;
            if locator.get("locator_version")?.as_u64()? != 1
                || !matches!(locator.get("kind")?.as_str()?, "text" | "text-continued")
            {
                return None;
            }
            identity("godot-dialogic", locator.get("speaker")?.as_str()?)
        }
        _ => None,
    }
}

#[derive(Debug, Serialize)]
pub struct SpeakerGroup {
    pub id: String,
    pub name: String,
    pub entry_ids: Vec<String>,
    pub count: usize,
    pub note: String,
}

#[derive(Debug, Default, Serialize)]
pub struct SpeakerGroups {
    pub groups: Vec<SpeakerGroup>,
    pub unassigned_entry_ids: Vec<String>,
}

/// Rebuild membership from current rows, never from saved notes or source text.
pub fn group_speakers(
    entries: &[StringEntry],
    format: &str,
    notes: &SpeakerNotes,
) -> SpeakerGroups {
    let mut groups = std::collections::BTreeMap::new();
    let mut result = SpeakerGroups::default();
    for entry in entries {
        if let Some(speaker) = resolve_speaker(entry, format) {
            let group = groups
                .entry(speaker.id.clone())
                .or_insert_with(|| SpeakerGroup {
                    note: notes.get(&speaker.id).cloned().unwrap_or_default(),
                    id: speaker.id,
                    name: speaker.name,
                    entry_ids: Vec::new(),
                    count: 0,
                });
            group.entry_ids.push(entry.id.clone());
            group.count += 1;
        } else {
            result.unassigned_entry_ids.push(entry.id.clone());
        }
    }
    result.groups = groups.into_values().collect();
    for group in &mut result.groups {
        group.entry_ids.sort();
    }
    result.unassigned_entry_ids.sort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn speakers_recognize_only_explicit_or_format_backed_identities() {
        let mut row = StringEntry::new("script.rpy#1", "Hello", "script.rpy".into())
            .with_context("alice")
            .with_tags(vec!["dialogue".into()]);
        assert_eq!(resolve_speaker(&row, "unknown"), None);
        let renpy = resolve_speaker(&row, "renpy").expect("Ren'Py say speaker");
        assert_eq!(renpy.name, "alice");
        assert_eq!(resolve_speaker(&row, ""), Some(renpy.clone()));
        row.metadata.insert("format_id".into(), json!("html"));
        assert_eq!(resolve_speaker(&row, "renpy"), None);
        assert_eq!(resolve_speaker(&row, ""), None);
        row.metadata.insert("speaker".into(), json!("Alice"));
        let explicit = resolve_speaker(&row, "html").unwrap();
        assert_eq!(explicit.name, "Alice");
        assert_ne!(explicit.id, renpy.id);
        row.metadata.insert("character".into(), json!("Bob"));
        assert_eq!(resolve_speaker(&row, "html"), None);
        row.metadata.clear();
        row.context = Some("Layout / scene".into());
        assert_eq!(resolve_speaker(&row, "renpy"), None);
        row.context = Some("alice".into());
        row.tags = vec!["name".into(), "speaker".into()];
        assert_eq!(resolve_speaker(&row, "renpy"), None);
        row.metadata
            .insert("format_id".into(), json!("godot-dialogic"));
        row.metadata.insert(
            "godot_dialogic".into(),
            json!({
                "locator_version": 1, "kind": "text", "speaker": "Alice"
            }),
        );
        assert_eq!(
            resolve_speaker(&row, "godot-dialogic").unwrap().name,
            "Alice"
        );
        row.metadata.remove("format_id");
        assert_eq!(resolve_speaker(&row, "").unwrap().name, "Alice");
        row.metadata.get_mut("godot_dialogic").unwrap()["locator_version"] = json!(2);
        assert_eq!(resolve_speaker(&row, "godot-dialogic"), None);
        row.metadata.clear();
        for key in ["speaker", "speaker_name", "character"] {
            row.metadata.insert(key.into(), json!("Alice"));
            assert_eq!(resolve_speaker(&row, "html"), Some(explicit.clone()));
            row.metadata.clear();
            row.metadata.insert(key.into(), json!("  "));
            assert_eq!(resolve_speaker(&row, "html"), None);
            row.metadata.clear();
        }
    }

    #[test]
    fn speakers_membership_tracks_rows_not_saved_notes_or_repeated_sources() {
        let a = StringEntry::new("a", "Same", "script.rpy".into())
            .with_context("alice")
            .with_tags(vec!["dialogue".into()]);
        let b = StringEntry {
            id: "b".into(),
            context: Some("bob".into()),
            ..a.clone()
        };
        let unknown = StringEntry::new("u", "Same", "other.txt".into()).with_context("alice");
        let notes = SpeakerNotes::from([("orphan".into(), "Retained but not a group".into())]);
        let grouped = group_speakers(&[a.clone(), b, unknown], "renpy", &notes);
        assert_eq!(grouped.groups.len(), 2);
        assert!(grouped
            .groups
            .iter()
            .all(|group| group.count == 1 && group.entry_ids.len() == 1));
        assert_eq!(grouped.unassigned_entry_ids, ["u"]);
        let removed = group_speakers(&[a], "renpy", &notes);
        assert_eq!(removed.groups.len(), 1);
        assert_eq!(removed.groups[0].entry_ids, ["a"]);
    }
}
