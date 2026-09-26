use locust_core::{extraction::FormatPlugin, models::StringEntry};
use std::path::{Component, Path};

/// Opt-in real-format test: extraction reads an explicitly supplied copy, while
/// injection only ever receives a new temporary fixture and relocated entry.
pub fn isolated_entry(
    plugin: &dyn FormatPlugin,
    env_name: &str,
    forbidden_original: &Path,
    required_extension: Option<&str>,
) -> (tempfile::TempDir, StringEntry) {
    let supplied = std::env::var_os(env_name).unwrap_or_else(|| {
        panic!("Set {env_name} to an explicit game COPY before running this ignored test")
    });
    let root = std::fs::canonicalize(supplied).expect("Game copy must exist");
    assert!(root.is_dir(), "Game copy must be a directory");
    if let Ok(original) = std::fs::canonicalize(forbidden_original) {
        assert!(
            !root.starts_with(&original) && !original.starts_with(&root),
            "Refusing the original game or any ancestor/descendant of it"
        );
    }

    // Fresh extraction avoids mutable project databases and stale absolute paths.
    // A same-byte-length case edit exercises writing without paid API calls or
    // introducing an artificial binary-slot overflow.
    let mut entry = plugin
        .extract(&root)
        .expect("Extract explicit game copy")
        .into_iter()
        .find(|entry| {
            entry.source.len() > 8
                && required_extension.is_none_or(|extension| {
                    entry
                        .file_path
                        .extension()
                        .is_some_and(|actual| actual == extension)
                        && !entry.tags.iter().any(|tag| tag == "rpyc")
                })
                && !entry.source.contains(['[', ']', '{', '}', '<', '>', '\\'])
                && entry.source.bytes().any(|byte| byte.is_ascii_lowercase())
        })
        .expect("Game has no plain text entry suitable for this write smoke test");
    let virtual_rel = entry
        .file_path
        .strip_prefix(&root)
        .expect("Extracted entry must be within the selected copy")
        .to_path_buf();
    assert!(virtual_rel
        .components()
        .all(|c| matches!(c, Component::Normal(_))));

    // UnityFS entries have virtual node components after the physical bundle.
    let backing = entry
        .file_path
        .ancestors()
        .find(|p| p.is_file())
        .expect("Extracted entry must have a physical backing file");
    let resolved_backing = std::fs::canonicalize(backing).expect("Resolve backing file");
    assert!(
        resolved_backing.starts_with(&root),
        "Backing file escapes game copy"
    );
    let backing_rel = backing
        .strip_prefix(&root)
        .expect("Backing file within copy");
    let fixture = tempfile::tempdir().expect("Create isolated injection fixture");
    let destination = fixture.path().join(backing_rel);
    std::fs::create_dir_all(destination.parent().unwrap()).expect("Create fixture parents");
    std::fs::copy(&resolved_backing, destination).expect("Copy backing file into fixture");
    entry.file_path = fixture.path().join(virtual_rel);

    let mut translation = entry.source.as_bytes().to_vec();
    let at = translation.iter().position(u8::is_ascii_lowercase).unwrap();
    translation[at].make_ascii_uppercase();
    entry.translation = Some(String::from_utf8(translation).unwrap());
    entry.status = locust_core::models::StringStatus::Translated;
    (fixture, entry)
}
