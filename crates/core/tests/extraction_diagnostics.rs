use std::path::Path;

use locust_core::{
    database::{Database, EntryFilter},
    error::Result,
    extraction::{Extraction, FormatPlugin, FormatRegistry, InjectionReport},
    models::{StringEntry, StringStatus},
    project::{extraction_warnings, open_project, open_project_db, saved_extraction_warnings},
};

struct LegacyPlugin(Vec<StringEntry>);

impl FormatPlugin for LegacyPlugin {
    fn id(&self) -> &str {
        "diagnostics-test"
    }
    fn name(&self) -> &str {
        "Diagnostics test"
    }
    fn supported_extensions(&self) -> &[&str] {
        &[]
    }
    fn detect(&self, _: &Path) -> bool {
        true
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        Ok(self.0.clone())
    }
    fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
        unreachable!("extraction-only fixture")
    }
}

struct DiagnosticPlugin(Extraction);

impl FormatPlugin for DiagnosticPlugin {
    fn id(&self) -> &str {
        "diagnostics-test"
    }
    fn name(&self) -> &str {
        "Diagnostics test"
    }
    fn supported_extensions(&self) -> &[&str] {
        &[]
    }
    fn detect(&self, _: &Path) -> bool {
        true
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        panic!("project opening must use the diagnostics channel")
    }
    fn extract_with_diagnostics(&self, _: &Path) -> Result<Extraction> {
        Ok(self.0.clone())
    }
    fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
        unreachable!("extraction-only fixture")
    }
}

fn registry(plugin: impl FormatPlugin + 'static) -> FormatRegistry {
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(plugin));
    registry
}

fn rows() -> Vec<StringEntry> {
    ["hero", "npc"]
        .into_iter()
        .map(|id| StringEntry::new(id, id, "story.txt".into()))
        .collect()
}

#[test]
fn legacy_default_retains_entries_and_exact_metadata_warnings() {
    let mut entries = rows();
    entries[0].metadata.insert(
        "iostore_unread_containers".into(),
        serde_json::json!(["z.utoc: encrypted", "a.utoc: unreadable", "", 42]),
    );
    entries[1].metadata.insert(
        "iostore_unread_containers".into(),
        serde_json::json!(["z.utoc: encrypted"]),
    );
    let plugin: &dyn FormatPlugin = &LegacyPlugin(entries.clone());
    let extraction = plugin
        .extract_with_diagnostics(Path::new("unused"))
        .unwrap();
    assert!(!extraction.complete);
    assert_eq!(extraction.warnings, extraction_warnings(&entries));
    assert_eq!(
        extraction.warnings,
        ["a.utoc: unreadable", "z.utoc: encrypted"]
    );
    assert_eq!(extraction.bounded_warnings(), extraction.warnings);
    assert_eq!(
        serde_json::to_value(extraction.entries).unwrap(),
        serde_json::to_value(plugin.extract(Path::new("unused")).unwrap()).unwrap()
    );
}

#[test]
fn legacy_default_is_complete_including_zero_rows() {
    for entries in [rows(), vec![]] {
        let extraction = LegacyPlugin(entries.clone())
            .extract_with_diagnostics(Path::new("unused"))
            .unwrap();
        assert!(extraction.complete);
        assert!(extraction.warnings.is_empty());
        assert_eq!(extraction.entries.len(), entries.len());
    }
}

#[test]
fn plugin_warnings_are_bounded_deduplicated_and_stably_ordered() {
    let mut warnings = vec!["z".into(), "a".into(), "z".into(), String::new()];
    warnings.push("é".repeat(2050));
    warnings.push("é".repeat(2048));
    warnings.extend((0..140).map(|i| format!("member-{i:03}: unreadable")));
    let extraction = Extraction {
        entries: vec![],
        warnings,
        complete: false,
    };
    let bounded = extraction.bounded_warnings();
    assert_eq!(bounded.len(), 128);
    assert!(bounded.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(bounded
        .iter()
        .all(|warning| warning.chars().count() <= 2048));
    assert!(bounded.contains(&"é".repeat(2048)));
    assert_eq!(bounded, extraction.bounded_warnings());
}

#[test]
fn incomplete_without_messages_still_has_a_persistable_warning() {
    let extraction = Extraction {
        entries: vec![],
        warnings: vec![String::new()],
        complete: false,
    };
    assert_eq!(extraction.bounded_warnings().len(), 1);
    assert!(extraction.bounded_warnings()[0].contains("incomplete"));
}

#[test]
fn incomplete_extraction_preserves_saved_rows_and_complete_zero_rows_clears_them() {
    // Exercise both zero rows and some rows, without Unreal entry metadata.
    for count in [0, 1] {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let db = Database::open_in_memory().unwrap();
        let first = open_project(&db, &registry(LegacyPlugin(rows())), &game, None).unwrap();
        let mut saved = db.get_entries(&EntryFilter::default()).unwrap();
        for (i, row) in saved.iter_mut().enumerate() {
            row.translation = Some(format!("Traducción {i}"));
            row.status = if i == 0 {
                StringStatus::Approved
            } else {
                StringStatus::Translated
            };
            row.provider_used = Some("manual".into());
            row.translated_at = Some(chrono::Utc::now());
            row.reviewed_at = Some(chrono::Utc::now());
        }
        db.save_entries(&saved).unwrap();
        let before =
            serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
        let warning = "timeline.dtl: escaped event prefix is unsupported";
        let partial_registry = registry(DiagnosticPlugin(Extraction {
            entries: rows().into_iter().take(count).collect(),
            warnings: vec![warning.into()],
            complete: false,
        }));
        let partial = open_project(&db, &partial_registry, &game, None).unwrap();
        assert_eq!(partial.total_strings, 2);
        assert_eq!(partial.removed, 0);
        assert_eq!(partial.extraction_warnings, [warning]);
        assert_eq!(
            serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            before
        );
        let reopened = open_project_db(
            &db,
            &partial_registry,
            &first.database_path,
            &game,
            "diagnostics-test",
        )
        .unwrap();
        assert_eq!(reopened.extraction_warnings, [warning]);
        assert_eq!(saved_extraction_warnings(&db).unwrap(), [warning]);

        // The unchanged legacy default is authoritative even when it finds no rows.
        let complete = open_project(&db, &registry(LegacyPlugin(vec![])), &game, None).unwrap();
        assert_eq!(complete.total_strings, 0);
        assert_eq!(complete.removed, 2);
        assert!(complete.extraction_warnings.is_empty());
        assert!(saved_extraction_warnings(&db).unwrap().is_empty());
    }
}
