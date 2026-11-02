use std::{fs, path::Path};

use locust_core::{
    database::Database,
    extraction::{FormatPlugin, FormatRegistry},
    models::StringStatus,
    project::{open_project, saved_extraction_warnings},
};
use locust_formats::{
    kirikiri::KirikiriPlugin,
    kirikiri_xp3::{write_xp3_custom, Xp3Archive},
    yuris::YurisPlugin,
    yuris_ypf::{write_ypf, YpfArchive},
};

#[derive(Clone, Copy)]
enum ArchiveKind {
    Xp3,
    Ypf,
}

impl ArchiveKind {
    fn plugin(self) -> Box<dyn FormatPlugin> {
        match self {
            Self::Xp3 => Box::new(KirikiriPlugin::new()),
            Self::Ypf => Box::new(YurisPlugin::new()),
        }
    }

    fn archive(self) -> &'static str {
        match self {
            Self::Xp3 => "data.xp3",
            Self::Ypf => "ysbin.ypf",
        }
    }

    fn member(self, name: &str) -> String {
        match self {
            Self::Xp3 => format!("scenario/{name}.ks"),
            Self::Ypf => format!("scripts/{name}.ybn"),
        }
    }

    fn row_id(self, name: &str, index: usize) -> String {
        let locator = match self {
            Self::Xp3 => format!("kag:{}", index + 1),
            Self::Ypf => format!("attr{index}"),
        };
        format!("{}/{}#{locator}", self.archive(), self.member(name))
    }

    fn payload(self, lines: &[&str]) -> Vec<u8> {
        match self {
            Self::Xp3 => format!("{}\n", lines.join("\n")).into_bytes(),
            Self::Ypf => ystb(lines),
        }
    }

    fn damaged_payload(self) -> Vec<u8> {
        match self {
            // Truncated KiriKiri mode-2 stream: fails text decoding.
            Self::Xp3 => vec![0xfe, 0xfe, 2, 0xff, 0xfe, 0, 0],
            Self::Ypf => b"YSTB".to_vec(),
        }
    }

    fn write(self, path: &Path, members: &[(String, Vec<u8>)]) {
        let files: Vec<_> = members
            .iter()
            .map(|(name, payload)| (name.clone(), payload.clone(), true))
            .collect();
        let bytes = match self {
            Self::Xp3 => write_xp3_custom(&files).unwrap(),
            Self::Ypf => write_ypf(0x1e4, 0xff, &files).unwrap(),
        };
        fs::write(path, bytes).unwrap();
    }

    fn damage_read(self, path: &Path, name: &str) {
        let offset = match self {
            Self::Xp3 => {
                let archive = Xp3Archive::open(path).unwrap();
                let offset = archive
                    .ks_entries()
                    .find(|entry| entry.name.replace('\\', "/") == name)
                    .unwrap()
                    .segments[0]
                    .offset as usize;
                offset
            }
            Self::Ypf => {
                let archive = YpfArchive::open(path).unwrap();
                let offset = archive
                    .ybn_entries()
                    .find(|entry| entry.name.replace('\\', "/") == name)
                    .unwrap()
                    .offset as usize;
                offset
            }
        };
        // Keep the index valid but break this member's zlib header.
        let mut bytes = fs::read(path).unwrap();
        bytes[offset] = 0;
        fs::write(path, bytes).unwrap();
    }
}

// Adapted from identity_matrix.rs / yuris.rs::build_ystb_with_inst_pad:
// XOR-encrypted 0x22b YSTB with one display expression per supplied ASCII line.
fn ystb(texts: &[&str]) -> Vec<u8> {
    let mut values = Vec::new();
    let mut descriptors = Vec::new();
    for text in texts {
        assert!(text.is_ascii() && !text.contains(['"', '\\']));
        let body = format!("\"{text}\"");
        let mut expression = vec![0x4d];
        expression.extend_from_slice(&(body.len() as u16).to_le_bytes());
        expression.extend_from_slice(body.as_bytes());
        descriptors.extend_from_slice(&0i16.to_le_bytes());
        descriptors.extend_from_slice(&3i16.to_le_bytes());
        descriptors.extend_from_slice(&(expression.len() as u32).to_le_bytes());
        descriptors.extend_from_slice(&(values.len() as u32).to_le_bytes());
        values.extend(expression);
    }
    values.extend_from_slice(&[0; 16]);
    while !values.len().is_multiple_of(4) {
        values.push(0);
    }
    let num_inst = texts.len() as u32 + 8;
    let mut instructions = [108, 1, 0, 0].repeat(texts.len());
    instructions.extend_from_slice(&[0; 32]);
    let lines: Vec<u8> = (1..=num_inst).flat_map(u32::to_le_bytes).collect();
    let mut bytes = b"YSTB".to_vec();
    for value in [
        0x22b,
        num_inst,
        instructions.len() as u32,
        descriptors.len() as u32,
        values.len() as u32,
        lines.len() as u32,
        0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let key = 0xb4626ad8u32.to_le_bytes();
    for section in [instructions, descriptors, values, lines] {
        bytes.extend(section.iter().enumerate().map(|(i, b)| b ^ key[i % 4]));
    }
    bytes
}

fn refresh_preserves_until_repaired(kind: ArchiveKind, read_failure: bool) {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join(kind.archive());
    let mut members = vec![
        (
            kind.member("a"),
            kind.payload(&["A kept", "A removed later"]),
        ),
        (kind.member("b"), kind.payload(&["B original"])),
    ];
    kind.write(&archive, &members);
    let db = Database::open_in_memory().unwrap();
    let mut registry = FormatRegistry::new();
    let plugin = kind.plugin();
    let format = plugin.id().to_owned();
    registry.register(plugin);
    let first = open_project(&db, &registry, dir.path(), Some(&format)).unwrap();
    assert_eq!(first.total_strings, 3);
    assert!(first.extraction_warnings.is_empty());

    let mut saved = Vec::new();
    for (name, index, translation) in [
        ("a", 0, "A guardada"),
        ("a", 1, "A para eliminar"),
        ("b", 0, "B guardada"),
    ] {
        let mut entry = db.get_entry(&kind.row_id(name, index)).unwrap().unwrap();
        assert_eq!(
            entry.file_path,
            Path::new(kind.archive()).join(kind.member(name))
        );
        entry.translation = Some(translation.into());
        entry.status = StringStatus::Approved;
        entry.provider_used = Some("manual".into());
        saved.push(entry);
    }
    db.save_entries(&saved).unwrap();

    members[1].1 = kind.payload(&["B updated", "B added"]);
    if !read_failure {
        members[0].1 = kind.damaged_payload();
    }
    kind.write(&archive, &members);
    if read_failure {
        kind.damage_read(&archive, &kind.member("a"));
    }
    let partial = open_project(&db, &registry, dir.path(), Some(&format)).unwrap();
    assert_eq!(partial.total_strings, 4);
    assert_eq!(partial.removed, 0);
    assert_eq!(partial.added, 1);
    assert_eq!(partial.stale_source_reset, 1);
    for entry in &saved[..2] {
        assert_eq!(
            serde_json::to_value(db.get_entry(&entry.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(entry).unwrap()
        );
    }
    let updated = db.get_entry(&saved[2].id).unwrap().unwrap();
    assert_eq!(updated.file_path, saved[2].file_path);
    assert_eq!(updated.source, "B updated");
    assert_eq!(updated.translation.as_deref(), Some("B guardada"));
    assert_eq!(updated.status, StringStatus::Pending);
    assert_eq!(updated.provider_used.as_deref(), Some("manual"));
    assert_eq!(
        db.get_entry(&kind.row_id("b", 1)).unwrap().unwrap().source,
        "B added"
    );
    assert_eq!(partial.extraction_warnings.len(), 1);
    let warning = &partial.extraction_warnings[0];
    assert!(warning.starts_with("skipped "), "{warning}");
    assert!(warning.contains(kind.archive()), "{warning}");
    assert!(warning.contains(&kind.member("a")), "{warning}");
    let reason = if read_failure {
        "decode"
    } else {
        match kind {
            ArchiveKind::Xp3 => "mode-2 missing size fields",
            ArchiveKind::Ypf => "header",
        }
    };
    assert!(warning.contains(reason), "{warning}");
    assert_eq!(
        saved_extraction_warnings(&db).unwrap(),
        partial.extraction_warnings
    );

    members[0].1 = kind.payload(&["A kept"]);
    kind.write(&archive, &members);
    let complete = open_project(&db, &registry, dir.path(), Some(&format)).unwrap();
    assert_eq!(complete.total_strings, 3);
    assert_eq!(complete.removed, 1);
    assert!(db.get_entry(&saved[1].id).unwrap().is_none());
    assert_eq!(
        db.get_entry(&saved[0].id).unwrap().unwrap().translation,
        saved[0].translation
    );
    assert!(complete.extraction_warnings.is_empty());
    assert!(saved_extraction_warnings(&db).unwrap().is_empty());
}

#[test]
fn kirikiri_undecodable_member_refresh_preserves_translations_until_repaired() {
    refresh_preserves_until_repaired(ArchiveKind::Xp3, false);
}

#[test]
fn kirikiri_member_read_failure_refresh_preserves_translations_until_repaired() {
    refresh_preserves_until_repaired(ArchiveKind::Xp3, true);
}

#[test]
fn yuris_truncated_member_refresh_preserves_translations_until_repaired() {
    refresh_preserves_until_repaired(ArchiveKind::Ypf, false);
}

#[test]
fn yuris_member_read_failure_refresh_preserves_translations_until_repaired() {
    refresh_preserves_until_repaired(ArchiveKind::Ypf, true);
}

#[test]
fn kirikiri_failed_member_with_readable_substitute_is_complete() {
    let kind = ArchiveKind::Xp3;
    // Both a hard decode error and ciphertext that decodes to control soup
    // must yield to a readable copy, including a script with no dialogue rows.
    for damaged in [kind.damaged_payload(), vec![1; 64]] {
        for patch in [false, true] {
            for text in ["Plaintext dialogue.\n", "; only a comment\n"] {
                let dir = tempfile::tempdir().unwrap();
                kind.write(
                    &dir.path().join("data.xp3"),
                    &[("scenario/first.ks".into(), damaged.clone())],
                );
                let rel = if patch {
                    kind.write(
                        &dir.path().join("patch.xp3"),
                        &[("FIRST.KS".into(), text.as_bytes().to_vec())],
                    );
                    "patch.xp3/FIRST.KS"
                } else {
                    fs::create_dir_all(dir.path().join("unencrypted/scenario")).unwrap();
                    fs::write(dir.path().join("unencrypted/scenario/FIRST.KS"), text).unwrap();
                    "unencrypted/scenario/FIRST.KS"
                };
                let plugin = kind.plugin();
                let extraction = plugin.extract_with_diagnostics(dir.path()).unwrap();
                assert!(extraction.complete, "{:?}", extraction.warnings);
                assert!(extraction.warnings.is_empty());
                if text.starts_with(';') {
                    assert!(extraction.entries.is_empty());
                } else {
                    assert_eq!(extraction.entries.len(), 1);
                    assert_eq!(extraction.entries[0].source, "Plaintext dialogue.");
                    assert_eq!(extraction.entries[0].id, format!("{rel}#kag:1"));
                    assert_eq!(
                        plugin.extract(dir.path()).unwrap()[0].id,
                        extraction.entries[0].id
                    );
                }
            }
        }
    }
}

fn unreadable_members_without_rows_are_incomplete(kind: ArchiveKind) {
    let dir = tempfile::tempdir().unwrap();
    kind.write(
        &dir.path().join(kind.archive()),
        &[(kind.member("a"), kind.damaged_payload())],
    );
    let plugin = kind.plugin();
    let extraction = plugin.extract_with_diagnostics(dir.path()).unwrap();
    assert!(!extraction.complete);
    assert!(extraction.entries.is_empty());
    assert_eq!(extraction.warnings.len(), 1);
    assert!(extraction.warnings[0].contains(kind.archive()));
    assert!(extraction.warnings[0].contains(&kind.member("a")));
    assert!(plugin.extract(dir.path()).unwrap().is_empty());
}

#[test]
fn kirikiri_unreadable_member_without_rows_is_incomplete() {
    unreadable_members_without_rows_are_incomplete(ArchiveKind::Xp3);
}

#[test]
fn yuris_unreadable_member_without_rows_is_incomplete() {
    unreadable_members_without_rows_are_incomplete(ArchiveKind::Ypf);
}

fn unreadable_archive_with_readable_archive_is_incomplete(kind: ArchiveKind) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(kind.archive()), b"broken archive").unwrap();
    let healthy = match kind {
        ArchiveKind::Xp3 => "other.xp3",
        ArchiveKind::Ypf => "other.ypf",
    };
    kind.write(
        &dir.path().join(healthy),
        &[(kind.member("b"), kind.payload(&["Readable dialogue."]))],
    );
    let extraction = kind.plugin().extract_with_diagnostics(dir.path()).unwrap();
    assert!(!extraction.complete);
    assert_eq!(extraction.entries.len(), 1);
    assert_eq!(extraction.entries[0].source, "Readable dialogue.");
    assert_eq!(extraction.warnings.len(), 1);
    assert!(extraction.warnings[0].contains(kind.archive()));
}

#[test]
fn kirikiri_unreadable_archive_alongside_readable_archive_is_incomplete() {
    unreadable_archive_with_readable_archive_is_incomplete(ArchiveKind::Xp3);
}

#[test]
fn yuris_unreadable_archive_alongside_readable_archive_is_incomplete() {
    unreadable_archive_with_readable_archive_is_incomplete(ArchiveKind::Ypf);
}

#[test]
fn kirikiri_different_script_key_does_not_cover_failed_member() {
    let dir = tempfile::tempdir().unwrap();
    let kind = ArchiveKind::Xp3;
    kind.write(
        &dir.path().join(kind.archive()),
        &[("scenario/select/first.ks".into(), vec![1; 64])],
    );
    fs::write(dir.path().join("first.ks"), "Other script.\n").unwrap();
    let extraction = kind.plugin().extract_with_diagnostics(dir.path()).unwrap();
    assert!(!extraction.complete);
    assert_eq!(extraction.entries.len(), 1);
    assert_eq!(extraction.entries[0].source, "Other script.");
    assert_eq!(extraction.warnings.len(), 1);
    assert!(extraction.warnings[0].contains("data.xp3/scenario/select/first.ks"));
}
