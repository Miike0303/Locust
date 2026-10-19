use super::super::tests::{make_entry, recording_tempdir, shared_textasset_rows};
use super::*;

#[test]
fn partial_extract_preserves_absent_rows_and_pivot_provenance() {
    let db = Database::open_in_memory().unwrap();
    let mut a = make_entry("a", "日本語A");
    a.translation = Some("English A".into());
    let mut b = make_entry("b", "日本語B");
    b.translation = Some("English B".into());
    db.save_entries(&[a.clone(), b]).unwrap();
    let stats = db.merge_entries_preserving_missing(&[a.clone()]).unwrap();
    assert_eq!(stats.removed, 0);
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("English B")
    );
    db.set_project_metadata(
        "extraction_warnings",
        &serde_json::json!(["Unread encrypted TOC"]),
    )
    .unwrap();
    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let path = dir.join("partial-pivot.db");
    db.pivot_to(&path).unwrap();
    let pivot = Database::open(&path).unwrap();
    assert_eq!(
        pivot.get_project_metadata("extraction_warnings").unwrap(),
        Some(serde_json::json!(["Unread encrypted TOC"]))
    );
    pivot.merge_entries_preserving_missing(&[a]).unwrap();
    assert_eq!(pivot.get_entry("b").unwrap().unwrap().source, "English B");
    assert!(pivot
        .merge_entries_preserving_missing(&[make_entry("a", "changed")])
        .is_err());
    assert_eq!(pivot.get_entry("a").unwrap().unwrap().source, "English A");
    assert!(pivot.merge_entries(&[]).is_err());
    assert_eq!(db.merge_entries(&[]).unwrap().removed, 2);
}

#[test]
fn shared_textasset_database_large_roundtrip_has_one_blob_and_shared_memory() {
    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let path = dir.join("shared.db");
    let (original, entries) = shared_textasset_rows(512);
    assert!(original.len() > 32_768);
    let db = Database::open(&path).unwrap();
    db.save_entries(&entries).unwrap();
    let conn = db.conn.lock().unwrap();
    let count: usize = conn
        .query_row("SELECT COUNT(*) FROM textasset_originals", [], |r| r.get(0))
        .unwrap();
    let metadata_size: usize = conn
        .query_row("SELECT MAX(length(metadata)) FROM strings", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    assert!(metadata_size < 1024);
    drop(conn);
    drop(db);
    let db = Database::open(&path).unwrap();
    let loaded = db.get_entries(&EntryFilter::default()).unwrap();
    let head = loaded[0].textasset_original.as_ref().unwrap();
    for row in &loaded {
        assert!(Arc::ptr_eq(head, row.textasset_original.as_ref().unwrap()));
        assert_eq!(
            crate::textasset_group::original_textasset(row),
            Some(original.as_str())
        );
        assert!(crate::textasset_group::parse_group_meta(row).is_ok());
    }
    // JSON carries the digest only; saving back into its owning DB rehydrates.
    let serialized = serde_json::to_string(&loaded[0]).unwrap();
    let detached: StringEntry = serde_json::from_str(&serialized).unwrap();
    assert!(detached.textasset_original.is_none());
    db.save_entries(&[detached]).unwrap();
}

#[test]
fn textasset_group_query_preserves_order_across_chunks_and_shared_originals() {
    let db = Database::open_in_memory().unwrap();
    let (original, mut entries) = shared_textasset_rows(601);
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.metadata.insert(
            crate::textasset_group::GROUP_ID_KEY.into(),
            serde_json::json!(if i % 2 == 0 { "group-a'" } else { "group-b" }),
        );
    }
    db.save_entries(&entries).unwrap();
    let expected = db.get_entries(&EntryFilter::default()).unwrap();
    let actual = db
        .get_entries_for_textasset_groups(&HashSet::from(["group-b".into(), "group-a'".into()]))
        .unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let head = actual[0].textasset_original.as_ref().unwrap();
    for entry in &actual {
        assert!(Arc::ptr_eq(
            head,
            entry.textasset_original.as_ref().unwrap()
        ));
        assert_eq!(head.text(), original);
    }
    assert!(db
        .get_entries_for_textasset_groups(&HashSet::new())
        .unwrap()
        .is_empty());
    assert!(db
        .get_entries_for_textasset_groups(&HashSet::from(["absent".into()]))
        .unwrap()
        .is_empty());
}

#[test]
fn textasset_group_query_rejects_corrupt_unselected_metadata() {
    let db = Database::open_in_memory().unwrap();
    let (_, entries) = shared_textasset_rows(3);
    db.save_entries(&entries).unwrap();
    db.save_entries(&[make_entry("unrelated", "Unrelated")])
        .unwrap();
    let selected = HashSet::from(["shared-group".into()]);
    let deep = format!("{{\"nested\":{}{}}}", "[".repeat(130), "]".repeat(130));
    for metadata in [
        "{",
        "[]",
        "null",
        "\"scalar\"",
        "{\"number\":1e9999}",
        &deep,
        "{\"textasset_group_original\":7}",
        "{\"textasset_group_original_ref\":7}",
        "{\"textasset_group_original_ref\":\"bad\"}",
        "{\"textasset_group_original_bytes\":1}",
    ] {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE strings SET metadata=?1 WHERE id='unrelated'",
                params![metadata],
            )
            .unwrap();
        assert!(
            db.get_entries(&EntryFilter::default()).is_err(),
            "{metadata}"
        );
        assert!(
            db.get_entries_for_textasset_groups(&selected).is_err(),
            "{metadata}"
        );
    }
    // serde uses the last duplicate key; SQLite json_extract uses the first.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE strings SET metadata=?1 WHERE id='unrelated'",
            params!["{\"textasset_group_id\":\"other\",\"textasset_group_id\":\"shared-group\"}"],
        )
        .unwrap();
    assert_eq!(
        db.get_entries_for_textasset_groups(&selected)
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn shared_textasset_partial_pivot_and_merge_preserve_physical_blob_without_leader() {
    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let (original, mut entries) = shared_textasset_rows(256);
    // Only the final row is translated: the group's first row never enters
    // either pivot, so blob ownership cannot depend on a leader row.
    entries.last_mut().unwrap().translation = Some("English last row".into());
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&entries).unwrap();
    let path = dir.join("pivot-one.db");
    db.pivot_to(&path).unwrap();
    let pivot = Database::open(&path).unwrap();
    let id = entries.last().unwrap().id.clone();
    let mut row = pivot.get_entry(&id).unwrap().unwrap();
    assert_eq!(row.source, "English last row");
    assert_eq!(
        crate::textasset_group::original_textasset(&row),
        Some(original.as_str())
    );
    assert_eq!(
        row.injection_source().unwrap(),
        entries.last().unwrap().source
    );
    row.translation = Some("Última fila española".into());
    pivot.save_entries(&[row]).unwrap();
    let path2 = dir.join("pivot-two.db");
    pivot.pivot_to(&path2).unwrap();
    let pivot2 = Database::open(&path2).unwrap();
    pivot2.merge_entries(&entries).unwrap();
    let row = pivot2.get_entry(&id).unwrap().unwrap();
    assert_eq!(row.source, "Última fila española");
    assert_eq!(
        crate::textasset_group::original_textasset(&row),
        Some(original.as_str())
    );
    let patch = crate::textasset_group::patch_from_entry(&row, "Texto final").unwrap();
    let meta = crate::textasset_group::parse_group_meta(&row).unwrap();
    let rebuilt = crate::textasset_group::apply_patches(&meta.original, meta.kind, &[patch]);
    assert!(rebuilt.outcomes[0].1.is_ok());
    assert!(rebuilt.text.starts_with("Menu.K0: 日本語"));
    assert!(rebuilt.text.contains("Menu.K255: Texto final"));
    // Change only an unselected row/padding, keeping the pivot row's source.
    let changed_original = original.replace("Menu.K0:", "Menu.X0:");
    crate::textasset_group::attach_to_entries(
        &mut entries,
        crate::textasset_group::GroupKind::LocLine,
        &changed_original,
        changed_original.len(),
        "shared-group",
    );
    assert!(pivot2
        .merge_entries(&entries)
        .unwrap_err()
        .to_string()
        .contains("source_changed"));
    assert_eq!(
        crate::textasset_group::original_textasset(&pivot2.get_entry(&id).unwrap().unwrap()),
        Some(original.as_str())
    );
}

#[test]
fn shared_textasset_legacy_inline_database_is_readable_and_migrates_on_save() {
    use crate::textasset_group as group;
    let db = Database::open_in_memory().unwrap();
    let (original, entries) = shared_textasset_rows(2);
    db.save_entries(&entries).unwrap();
    let mut legacy_metadata = entries[0].metadata.clone();
    legacy_metadata.remove(group::GROUP_ORIGINAL_REF_KEY);
    legacy_metadata.remove(group::GROUP_ORIGINAL_BYTES_KEY);
    legacy_metadata.insert(
        group::GROUP_ORIGINAL_KEY.into(),
        serde_json::json!(original),
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE strings SET metadata=?1",
            params![serde_json::to_string(&legacy_metadata).unwrap()],
        )
        .unwrap();
        conn.execute("DELETE FROM textasset_originals", []).unwrap();
    }
    let loaded = db.get_entries(&EntryFilter::default()).unwrap();
    assert!(Arc::ptr_eq(
        loaded[0].textasset_original.as_ref().unwrap(),
        loaded[1].textasset_original.as_ref().unwrap()
    ));
    db.save_entries(&loaded).unwrap();
    let conn = db.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM textasset_originals", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        1
    );
    assert!(!conn
        .query_row("SELECT metadata FROM strings LIMIT 1", [], |r| r
            .get::<_, String>(0))
        .unwrap()
        .contains("日本語"));
}

#[test]
fn shared_textasset_corrupt_missing_and_oversized_blobs_fail_closed() {
    for attack in ["payload", "length", "missing", "oversized", "reference"] {
        let db = Database::open_in_memory().unwrap();
        let (_, entries) = shared_textasset_rows(2);
        db.save_entries(&entries).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            match attack {
                "payload" => {
                    conn.execute("UPDATE textasset_originals SET payload='corrupt'", [])
                        .unwrap();
                }
                "length" => {
                    conn.execute("UPDATE textasset_originals SET byte_len=1", [])
                        .unwrap();
                }
                "missing" => {
                    conn.execute("DELETE FROM textasset_originals", []).unwrap();
                }
                "oversized" => {
                    conn.execute(
                        "UPDATE textasset_originals SET payload=?1",
                        params!["x".repeat(1_048_577)],
                    )
                    .unwrap();
                }
                "reference" => {
                    conn.execute("UPDATE strings SET metadata=json_set(metadata, '$.textasset_group_original_ref', 'bad')", []).unwrap();
                }
                _ => unreachable!(),
            }
        }
        assert!(db.get_entry(&entries[0].id).is_err(), "{attack}");
        assert!(db.get_entries(&EntryFilter::default()).is_err(), "{attack}");
        assert!(
            db.get_entries_for_textasset_groups(&HashSet::from(["unselected".into()]))
                .is_err(),
            "unselected original must still fail: {attack}"
        );
        if attack != "missing" && attack != "reference" {
            assert!(
                db.save_entries(&entries).is_err(),
                "save must not hide corrupt existing blob: {attack}"
            );
        }
    }
}

#[tokio::test]
async fn test_merge_entries_preserves_translations_and_status() {
    let db = Database::open_in_memory().unwrap();
    let mut first = make_entry("hero", "Hello");
    first.context = Some("old-ctx".into());
    db.save_entries(&[first, make_entry("gone", "Disappears")])
        .unwrap();
    assert!(db.save_translation("hero", "Hola", "mock").await.unwrap());
    db.update_entry_status("hero", StringStatus::Approved)
        .await
        .unwrap();

    let mut refreshed = make_entry("hero", "Hello");
    refreshed.context = Some("new-ctx".into());
    refreshed.file_path = PathBuf::from("data/Actors.json");
    refreshed.tags = vec!["ui".into()];
    let fresh = make_entry("mage", "Spell");
    let stats = db.merge_entries(&[refreshed, fresh]).unwrap();

    assert_eq!(stats.added, 1);
    assert_eq!(stats.updated, 1);
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.stale_source_reset, 0);
    assert_eq!(stats.preserved_translations, 1);
    assert_eq!(stats.lost_translations, 0);

    let hero = db.get_entry("hero").unwrap().unwrap();
    assert_eq!(hero.translation.as_deref(), Some("Hola"));
    assert_eq!(hero.status, StringStatus::Approved);
    assert_eq!(hero.provider_used.as_deref(), Some("mock"));
    assert_eq!(hero.context.as_deref(), Some("new-ctx"));
    assert_eq!(hero.file_path, PathBuf::from("data/Actors.json"));
    assert_eq!(hero.tags, vec!["ui".to_string()]);
    assert!(hero.translated_at.is_some());

    assert!(db.get_entry("gone").unwrap().is_none());
    let mage = db.get_entry("mage").unwrap().unwrap();
    assert_eq!(mage.status, StringStatus::Pending);
    assert!(mage.translation.is_none());
}

#[test]
fn merge_entries_writes_only_changed_rows() {
    let db = Database::open_in_memory().unwrap();
    let unchanged = make_entry("unchanged", "Hello");
    let mut changed = make_entry("changed", "Goodbye");
    db.save_entries(&[unchanged.clone(), changed.clone()])
        .unwrap();

    changed.context = Some("new context".into());
    let before: i64 = lock_connection(&db.conn)
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    let stats = db
        .merge_entries(&[unchanged.clone(), changed.clone()])
        .unwrap();
    let after: i64 = lock_connection(&db.conn)
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stats.updated, 1);
    assert_eq!(after - before, 1, "only the changed row should be written");

    let stats = db.merge_entries(&[unchanged, changed]).unwrap();
    let final_changes: i64 = lock_connection(&db.conn)
        .query_row("SELECT total_changes()", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stats.updated, 0);
    assert_eq!(
        final_changes - after,
        0,
        "an unchanged reopen should write no rows"
    );
}

#[tokio::test]
async fn test_merge_entries_stale_source_keeps_translation_resets_status() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("npc", "Welcome")]).unwrap();
    assert!(db
        .save_translation("npc", "Bienvenido", "mock")
        .await
        .unwrap());
    db.update_entry_status("npc", StringStatus::Approved)
        .await
        .unwrap();

    let stats = db
        .merge_entries(&[make_entry("npc", "Welcome, traveler")])
        .unwrap();
    assert_eq!(stats.stale_source_reset, 1);
    assert_eq!(stats.updated, 1);
    assert_eq!(stats.preserved_translations, 1);
    assert_eq!(stats.lost_translations, 1);
    assert_eq!(stats.added, 0);
    assert_eq!(stats.removed, 0);

    let npc = db.get_entry("npc").unwrap().unwrap();
    assert_eq!(npc.source, "Welcome, traveler");
    assert_eq!(npc.translation.as_deref(), Some("Bienvenido"));
    assert_eq!(npc.status, StringStatus::Pending);
    assert_eq!(npc.provider_used.as_deref(), Some("mock"));
}

#[tokio::test]
async fn merge_entries_counts_translations_lost_to_removal_or_pending_reset() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[
        make_entry("reset", "Hello"),
        make_entry("drop", "Goodbye"),
        make_entry("plain", "Stay"),
    ])
    .unwrap();
    assert!(db.save_translation("reset", "Hola", "mock").await.unwrap());
    assert!(db.save_translation("drop", "Adiós", "mock").await.unwrap());

    let stats = db
        .merge_entries(&[make_entry("reset", "Hello!"), make_entry("plain", "Stay")])
        .unwrap();
    assert_eq!(stats.stale_source_reset, 1);
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.lost_translations, 2);
    assert_eq!(stats.preserved_translations, 1);

    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("reset", "Hello"), make_entry("keep", "Goodbye")])
        .unwrap();
    assert!(db.save_translation("reset", "Hola", "mock").await.unwrap());
    assert!(db.save_translation("keep", "Adiós", "mock").await.unwrap());
    let stats = db
        .merge_entries_preserving_missing(&[make_entry("reset", "Hello!")])
        .unwrap();
    assert_eq!(stats.removed, 0);
    assert_eq!(stats.stale_source_reset, 1);
    assert_eq!(stats.lost_translations, 1);
}

#[test]
fn test_save_and_get_entries() {
    let db = Database::open_in_memory().unwrap();
    let entries = vec![
        make_entry("a", "Hello"),
        make_entry("b", "World"),
        make_entry("c", "Test"),
    ];
    let count = db.save_entries(&entries).unwrap();
    assert_eq!(count, 3);
    let all = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(all.len(), 3);
}

#[test]
fn test_save_entries_deduplication() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("dup", "First")]).unwrap();
    db.save_entries(&[make_entry("dup", "Second")]).unwrap();
    let all = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].source, "Second");
}

#[test]
fn test_filter_by_status() {
    let db = Database::open_in_memory().unwrap();
    let mut translated = make_entry("t1", "Translated one");
    translated.status = StringStatus::Translated;
    db.save_entries(&[
        make_entry("p1", "Pending one"),
        make_entry("p2", "Pending two"),
        translated,
    ])
    .unwrap();
    let filter = EntryFilter {
        status: Some(StringStatus::Pending),
        ..Default::default()
    };
    let results = db.get_entries(&filter).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_filter_by_search() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("s1", "hello world"), make_entry("s2", "goodbye")])
        .unwrap();
    let filter = EntryFilter {
        search: Some("hello".to_string()),
        ..Default::default()
    };
    let results = db.get_entries(&filter).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "s1");
}

#[test]
fn test_filter_limit_offset() {
    let db = Database::open_in_memory().unwrap();
    let entries: Vec<StringEntry> = (0..5)
        .map(|i| make_entry(&format!("e{}", i), &format!("Entry {}", i)))
        .collect();
    db.save_entries(&entries).unwrap();
    let filter = EntryFilter {
        limit: Some(2),
        offset: Some(2),
        ..Default::default()
    };
    let results = db.get_entries(&filter).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id, "e2");
    assert_eq!(results[1].id, "e3");
}

#[test]
fn test_get_entries_page_strings_query_count() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[
        make_entry("c", "hello three"),
        make_entry("a", "hello one"),
        make_entry("b", "hello two"),
        make_entry("d", "unrelated"),
    ])
    .unwrap();
    for (search, limit, offset, expected_len, expected_total, expected_queries) in [
        ("hello", 1, 1, 1, 3, 1),
        ("hello", 1, 3, 0, 3, 2),
        ("hello", 1, 30, 0, 3, 2),
        ("missing", 1, 0, 0, 0, 2),
        ("hello", 0, 0, 0, 3, 2),
    ] {
        let filter = EntryFilter {
            search: Some(search.into()),
            limit: Some(limit),
            offset: Some(offset),
            ..Default::default()
        };
        STRINGS_QUERIES.with(|count| count.set(Some(0)));
        let result = db.get_entries_page(&filter);
        let queries = STRINGS_QUERIES.with(|count| count.replace(None));
        let (entries, total) = result.unwrap();
        assert_eq!(
            queries,
            Some(expected_queries),
            "strings query count for {filter:?}"
        );
        assert_eq!(entries.len(), expected_len, "{filter:?}");
        assert_eq!(total, expected_total, "{filter:?}");
    }
}

#[test]
fn test_get_entries_page_matches_entries_and_count() {
    let db = Database::open_in_memory().unwrap();
    let mut a = make_entry("a", "source-only needle");
    a.file_path = PathBuf::from("data/menu's.json");
    a.tags = vec!["ui".into(), "dialogue".into()];
    let mut b = make_entry("b", "Other source");
    b.translation = Some("translation-only needle".into());
    b.status = StringStatus::Translated;
    b.file_path = PathBuf::from("data/other.json");
    b.tags = vec!["ui_label".into()];
    let mut c = a.clone();
    c.id = "c".into();
    c.source = "Another needle".into();
    c.translation = Some("Listo".into());
    c.status = StringStatus::Translated;
    c.context = Some("Menu title".into());
    c.char_limit = Some(42);
    c.provider_used = Some("test-provider".into());
    c.translated_at = Some(Utc::now());
    c.reviewed_at = Some(Utc::now());
    c.metadata
        .insert("custom".into(), serde_json::json!([1, "two"]));
    let mut d = make_entry("d", "Unrelated");
    d.status = StringStatus::Approved;
    db.save_entries(&[d, c, b, a]).unwrap();

    let cases: Vec<(EntryFilter, &[&str], usize)> = vec![
        (EntryFilter::default(), &["a", "b", "c", "d"], 4),
        (
            EntryFilter {
                status: Some(StringStatus::Translated),
                ..Default::default()
            },
            &["b", "c"],
            2,
        ),
        (
            EntryFilter {
                file_path: Some("data/menu's.json".into()),
                ..Default::default()
            },
            &["a", "c"],
            2,
        ),
        (
            EntryFilter {
                tag: Some("ui".into()),
                ..Default::default()
            },
            &["a", "c"],
            2,
        ),
        (
            EntryFilter {
                search: Some("source-only".into()),
                ..Default::default()
            },
            &["a"],
            1,
        ),
        (
            EntryFilter {
                search: Some("translation-only".into()),
                ..Default::default()
            },
            &["b"],
            1,
        ),
        (
            EntryFilter {
                search: Some("missing".into()),
                ..Default::default()
            },
            &[],
            0,
        ),
        (
            EntryFilter {
                limit: Some(2),
                offset: Some(1),
                ..Default::default()
            },
            &["b", "c"],
            4,
        ),
        (
            EntryFilter {
                limit: Some(2),
                offset: Some(4),
                ..Default::default()
            },
            &[],
            4,
        ),
        (
            EntryFilter {
                limit: Some(2),
                offset: Some(9),
                ..Default::default()
            },
            &[],
            4,
        ),
        (
            EntryFilter {
                offset: Some(2),
                ..Default::default()
            },
            &["c", "d"],
            4,
        ),
        (
            EntryFilter {
                limit: Some(0),
                ..Default::default()
            },
            &[],
            4,
        ),
        (
            EntryFilter {
                search: Some("needle".into()),
                limit: Some(1),
                offset: Some(1),
                ..Default::default()
            },
            &["b"],
            3,
        ),
        (
            EntryFilter {
                search: Some("needle".into()),
                limit: Some(1),
                offset: Some(3),
                ..Default::default()
            },
            &[],
            3,
        ),
        (
            EntryFilter {
                search: Some("needle".into()),
                offset: Some(9),
                ..Default::default()
            },
            &[],
            3,
        ),
        (
            EntryFilter {
                search: Some("source%only".into()),
                ..Default::default()
            },
            &["a"],
            1,
        ),
        (
            EntryFilter {
                search: Some(String::new()),
                ..Default::default()
            },
            &["a", "b", "c", "d"],
            4,
        ),
        (
            EntryFilter {
                status: Some(StringStatus::Translated),
                file_path: Some("data/menu's.json".into()),
                tag: Some("ui".into()),
                search: Some("needle".into()),
                limit: Some(1),
                offset: Some(0),
            },
            &["c"],
            1,
        ),
    ];
    for (filter, ids, expected_total) in cases {
        let expected_entries = db.get_entries(&filter).unwrap();
        let count = db.count_entries(&filter).unwrap();
        let (entries, total) = db.get_entries_page(&filter).unwrap();
        assert_eq!(total, count, "{filter:?}");
        assert_eq!(total, expected_total, "{filter:?}");
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ids,
            "{filter:?}"
        );
        assert_eq!(
            serde_json::to_value(&entries).unwrap(),
            serde_json::to_value(&expected_entries).unwrap(),
            "{filter:?}"
        );
    }
    let empty = Database::open_in_memory().unwrap();
    let (entries, total) = empty.get_entries_page(&EntryFilter::default()).unwrap();
    assert!(entries.is_empty());
    assert_eq!(total, 0);
}

#[test]
fn test_get_entries_page_preserves_shared_originals_and_errors() {
    let db = Database::open_in_memory().unwrap();
    let (original, entries) = shared_textasset_rows(4);
    db.save_entries(&entries).unwrap();
    let filter = EntryFilter {
        limit: Some(2),
        offset: Some(1),
        ..Default::default()
    };
    let expected = db.get_entries(&filter).unwrap();
    let (page, total) = db.get_entries_page(&filter).unwrap();
    assert_eq!(total, 4);
    assert_eq!(page.len(), 2);
    assert_eq!(
        serde_json::to_value(&page).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let head = page[0].textasset_original.as_ref().unwrap();
    for entry in &page {
        let restored = entry.textasset_original.as_ref().unwrap();
        assert!(Arc::ptr_eq(head, restored));
        assert_eq!(restored.text(), original);
    }
    for metadata in ["{", r#"{"textasset_group_original_ref":"missing"}"#] {
        lock_connection(&db.conn)
            .execute(
                "UPDATE strings SET metadata = ?1 WHERE id = ?2",
                params![metadata, page[0].id],
            )
            .unwrap();
        assert_eq!(
            db.get_entries_page(&filter).unwrap_err().to_string(),
            db.get_entries(&filter).unwrap_err().to_string()
        );
    }
}

#[tokio::test]
async fn test_save_translation_updates_status() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("tr1", "Hello")]).unwrap();
    assert!(db
        .save_translation("tr1", "Hola", "test-provider")
        .await
        .unwrap());
    let entry = db.get_entry("tr1").unwrap().unwrap();
    assert_eq!(entry.translation, Some("Hola".to_string()));
    assert_eq!(entry.status, StringStatus::Translated);
    assert_eq!(entry.provider_used, Some("test-provider".to_string()));
}

#[tokio::test]
async fn test_save_translation_unknown_id_returns_false() {
    let db = Database::open_in_memory().unwrap();
    assert!(!db
        .save_translation("missing", "Hola", "import")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_save_translations_batch_applies_known_skips_unknown() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("a", "Hello"), make_entry("b", "World")])
        .unwrap();
    let applied = db
        .save_translations_batch(
            vec![
                ("a".into(), "Hola".into()),
                ("missing".into(), "X".into()),
                ("b".into(), "Mundo".into()),
            ],
            "batch",
        )
        .await
        .unwrap();
    assert_eq!(applied, 2);
    assert_eq!(
        db.get_entry("a").unwrap().unwrap().translation.as_deref(),
        Some("Hola")
    );
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("Mundo")
    );
    assert!(db.get_entry("missing").unwrap().is_none());
}

#[tokio::test]
async fn guarded_batch_preserves_intervening_edits_and_applies_other_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("project.db");
    let db = Database::open(&path).unwrap();
    let desktop = Database::open(&path).unwrap();
    db.save_entries(&[make_entry("a", "A"), make_entry("b", "B")])
        .unwrap();
    db.save_translation("a", "old", "initial").await.unwrap();
    let expected = db.get_entry("a").unwrap().unwrap().translation;
    desktop
        .save_translation("a", "desktop edit", "manual")
        .await
        .unwrap();
    desktop
        .update_entry_status("a", StringStatus::Approved)
        .await
        .unwrap();
    let before = serde_json::to_value(db.get_entry("a").unwrap().unwrap()).unwrap();
    let report = db
        .save_translations_batch_if_unchanged(
            vec![
                TranslationBatchItem {
                    id: "a".into(),
                    translation: "stale replacement".into(),
                    expected_translation: Some(expected),
                },
                TranslationBatchItem {
                    id: "b".into(),
                    translation: "filled".into(),
                    expected_translation: Some(None),
                },
                TranslationBatchItem {
                    id: "missing".into(),
                    translation: "unknown".into(),
                    expected_translation: Some(None),
                },
            ],
            "batch",
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(db.get_entry("a").unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::json!({
            "requested":3, "applied":1, "skipped":2, "conflicts":["a"]
        })
    );
    let saved = db.get_entry("b").unwrap().unwrap();
    assert_eq!(saved.translation.as_deref(), Some("filled"));
    assert_eq!(saved.provider_used.as_deref(), Some("batch"));
    assert_eq!(saved.status, StringStatus::Translated);
}

#[tokio::test]
async fn guarded_batch_distinguishes_absent_null_and_empty_guards() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[
        make_entry("null", "A"),
        make_entry("empty", "B"),
        make_entry("legacy", "C"),
    ])
    .unwrap();
    db.save_translation("empty", "", "manual").await.unwrap();
    db.save_translation("legacy", "desktop edit", "manual")
        .await
        .unwrap();
    let updates = serde_json::from_value(serde_json::json!([
        {"id":"empty", "translation":"stale", "expected_translation":null},
        {"id":"null", "translation":"stale", "expected_translation":""},
        {"id":"legacy", "translation":"unconditional"},
        {"id":"empty", "translation":"filled empty", "expected_translation":""},
        {"id":"null", "translation":"filled null", "expected_translation":null}
    ]))
    .unwrap();
    let report = db
        .save_translations_batch_if_unchanged(updates, "batch")
        .await
        .unwrap();
    assert_eq!(report.conflicts, ["empty", "null"]);
    assert_eq!(
        (report.requested, report.applied, report.skipped),
        (5, 3, 2)
    );
    for (id, expected) in [
        ("legacy", "unconditional"),
        ("empty", "filled empty"),
        ("null", "filled null"),
    ] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().translation.as_deref(),
            Some(expected)
        );
    }
}

#[tokio::test]
async fn guarded_batch_rolls_back_on_error() {
    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[make_entry("a", "A"), make_entry("b", "B")])
        .unwrap();
    lock_connection(&db.conn)
        .execute_batch(
            "CREATE TRIGGER reject_second BEFORE UPDATE ON strings
            WHEN NEW.id = 'b' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        )
        .unwrap();
    let updates = ["a", "b"]
        .into_iter()
        .map(|id| TranslationBatchItem {
            id: id.into(),
            translation: "filled".into(),
            expected_translation: Some(None),
        })
        .collect();
    assert!(db
        .save_translations_batch_if_unchanged(updates, "batch")
        .await
        .is_err());
    for id in ["a", "b"] {
        assert!(db.get_entry(id).unwrap().unwrap().translation.is_none());
    }
}

#[tokio::test]
async fn test_export_import_po_multi_hash_id_through_db() {
    use crate::export::{export_po, import_po};
    use crate::models::StringEntry;
    use std::path::PathBuf;

    let db = Database::open_in_memory().unwrap();
    let id = "S004b.ks.json#0#message";
    let mut entry = StringEntry::new(id, "Hello there", PathBuf::from(r"C:\work\S004b.ks.json"));
    entry.translation = Some("PLACEHOLDER".to_string());
    db.save_entries(&[entry]).unwrap();

    // External CAT tool "edits" the PO.
    let mut e = db.get_entry(id).unwrap().unwrap();
    e.translation = Some("Hola alli".to_string());
    let po = export_po(std::slice::from_ref(&e), "en", "es");
    let imported = import_po(&po).unwrap();
    assert_eq!(imported[0].id.as_deref(), Some(id));

    let (updates, pre_skipped) = crate::export::po_entries_for_batch(&imported);
    let attempted = updates.len();
    let report = db.save_imported_translations_batch(updates).await.unwrap();
    let (imported_n, missed) =
        crate::export::import_counts_after_batch(pre_skipped, attempted, report.imported);
    assert_eq!(imported_n, 1);
    assert_eq!(missed, 0);
    let again = db.get_entry(id).unwrap().unwrap();
    assert_eq!(again.translation.as_deref(), Some("Hola alli"));
    assert_eq!(again.status, StringStatus::Translated);
}

fn import_rows_bytes(db: &Database) -> Vec<u8> {
    // JSON values sort metadata keys, so fresh HashMaps serialize identically.
    let rows = serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    serde_json::to_vec(&rows).unwrap()
}

fn identical_import_catalog() -> (Database, Vec<crate::export::ImportedTranslation>) {
    let db = Database::open_in_memory().unwrap();
    let entries: Vec<_> = (0..2_000)
        .rev()
        .map(|i| reviewed_import_entry(&format!("row{i:04}"), "Hello"))
        .collect();
    db.save_entries(&entries).unwrap();
    let updates = entries
        .into_iter()
        .map(|entry| crate::export::ImportedTranslation {
            id: entry.id,
            source: entry.source,
            translation: entry.translation.unwrap(),
        })
        .collect();
    (db, updates)
}

#[test]
fn import_preview_reads_at_most_four_chunks_for_2000_identical_rows() {
    let (db, updates) = identical_import_catalog();
    let before = import_rows_bytes(&db);
    IMPORT_READ_QUERIES.with(|count| count.set(0));
    let preview = db.preview_imported_translations(&updates, false).unwrap();
    let reads = IMPORT_READ_QUERIES.with(|count| count.replace(0));
    assert!(
        (1..=4).contains(&reads),
        "preview issued {reads} SELECTs against strings for 2000 rows; expected at most 4"
    );
    assert_eq!(
        serde_json::to_value(preview).unwrap(),
        serde_json::json!({
            "report": {
                "imported": 2000, "unchanged": 2000, "kept_existing": 0,
                "stale_sources": 0, "unknown_ids": 0,
            },
            "replacements": [], "confirmations": [], "fill_ids": [], "kept_ids": [],
            "unchanged_ids": updates.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        })
    );
    assert_eq!(import_rows_bytes(&db), before);
}

#[tokio::test]
async fn import_save_reads_at_most_four_chunks_for_2000_identical_rows() {
    let (db, updates) = identical_import_catalog();
    let before = import_rows_bytes(&db);
    IMPORT_READ_QUERIES.with(|count| count.set(0));
    let report = db
        .save_imported_translations_batch_with(updates, false)
        .await
        .unwrap();
    let reads = IMPORT_READ_QUERIES.with(|count| count.replace(0));
    assert!(
        (1..=4).contains(&reads),
        "save issued {reads} SELECTs against strings for 2000 rows; expected at most 4"
    );
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::json!({
            "imported": 2000, "unchanged": 2000, "kept_existing": 0,
            "stale_sources": 0, "unknown_ids": 0,
        })
    );
    assert_eq!(import_rows_bytes(&db), before);
}

#[tokio::test]
async fn import_reads_all_1201_rows_across_chunk_boundaries_in_catalog_order() {
    use crate::export::ImportedTranslation;

    let db = Database::open_in_memory().unwrap();
    let updates: Vec<_> = (0..1_201)
        .rev()
        .map(|i| ImportedTranslation {
            // Quotes and punctuation must remain bound parameters.
            id: format!("row'{i:04},?"),
            source: format!("Source {i}"),
            translation: format!("Translation {i}"),
        })
        .collect();
    db.save_entries(
        &updates
            .iter()
            .map(|entry| make_entry(&entry.id, &entry.source))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let expected_report = serde_json::json!({
        "imported": 1201, "unchanged": 0, "kept_existing": 0,
        "stale_sources": 0, "unknown_ids": 0,
    });
    IMPORT_READ_QUERIES.with(|count| count.set(0));
    let preview = db.preview_imported_translations(&updates, true).unwrap();
    assert_eq!(IMPORT_READ_QUERIES.with(|count| count.replace(0)), 3);
    assert_eq!(
        serde_json::to_value(preview).unwrap(),
        serde_json::json!({
            "report": expected_report,
            "replacements": [], "confirmations": [], "kept_ids": [], "unchanged_ids": [],
            "fill_ids": updates.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
        })
    );
    let report = db
        .save_imported_translations_batch_with(updates.clone(), true)
        .await
        .unwrap();
    assert_eq!(IMPORT_READ_QUERIES.with(|count| count.replace(0)), 3);
    assert_eq!(serde_json::to_value(report).unwrap(), expected_report);
    for update in &updates {
        let saved = db.get_entry(&update.id).unwrap().unwrap();
        assert_eq!(saved.source, update.source);
        assert_eq!(saved.translation.as_ref(), Some(&update.translation));
        assert_eq!(saved.status, StringStatus::Translated);
        assert_eq!(saved.provider_used.as_deref(), Some("import"));
    }
}

#[tokio::test]
async fn import_save_does_not_count_ignored_writes() {
    let db = Database::open_in_memory().unwrap();
    let mut confirm = reviewed_import_entry("confirm", "Hello again");
    confirm.status = StringStatus::Pending;
    let skipped = [
        make_entry("fill", "Goodbye"),
        confirm,
        reviewed_import_entry("replace", "Welcome"),
    ];
    db.save_entries(&skipped).unwrap();
    db.save_entries(&[make_entry("saved", "Thanks")]).unwrap();
    lock_connection(&db.conn)
        .execute_batch(
            "CREATE TRIGGER ignore_import BEFORE UPDATE ON strings
                 WHEN OLD.id IN ('fill', 'confirm', 'replace')
                 BEGIN SELECT RAISE(IGNORE); END;",
        )
        .unwrap();
    let updates = [
        ("fill", "Goodbye", "Adiós"),
        ("confirm", "Hello again", "Hola"),
        ("replace", "Welcome", "Bienvenido"),
        ("saved", "Thanks", "Gracias"),
    ]
    .into_iter()
    .map(
        |(id, source, translation)| crate::export::ImportedTranslation {
            id: id.into(),
            source: source.into(),
            translation: translation.into(),
        },
    )
    .collect();
    let report = db
        .save_imported_translations_batch_with(updates, false)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::json!({
            "imported": 1, "unchanged": 0, "kept_existing": 0,
            "stale_sources": 0, "unknown_ids": 0,
        })
    );
    for before in skipped {
        assert_eq!(
            serde_json::to_value(db.get_entry(&before.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap(),
        );
    }
    assert_eq!(
        db.get_entry("saved")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Gracias")
    );
}

#[tokio::test]
async fn import_preview_matches_apply_without_changing_any_rows() {
    use crate::export::ImportedTranslation;

    for keep_existing in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let mut confirm = reviewed_import_entry("confirm", "Hello again");
        confirm.status = StringStatus::Pending;
        let mut blank = make_entry("blank", "Thanks");
        blank.translation = Some(" \t\r\n\u{2003}".into());
        let mut same = reviewed_import_entry("same", "Hello");
        same.status = StringStatus::Translated;
        db.save_entries(&[
            same,
            confirm,
            make_entry("empty", "Goodbye"),
            blank,
            reviewed_import_entry("different", "Welcome"),
            reviewed_import_entry("stale", "Current source"),
            reviewed_import_entry("untouched", "Outside the catalog"),
        ])
        .unwrap();
        let updates: Vec<_> = [
            ("unknown", "Unknown", "Missing"),
            ("stale", "Old source", "Hola"),
            ("same", "Hello", "Hola"),
            ("confirm", "Hello again", "Hola"),
            ("empty", "Goodbye", "Adiós"),
            ("blank", "Thanks", "Gracias"),
            ("different", "Welcome", "Bienvenido"),
        ]
        .into_iter()
        .map(|(id, source, translation)| ImportedTranslation {
            id: id.into(),
            source: source.into(),
            translation: translation.into(),
        })
        .collect();
        let before = import_rows_bytes(&db);
        lock_connection(&db.conn)
            .execute_batch("PRAGMA query_only = ON")
            .unwrap();

        let preview = db
            .preview_imported_translations(&updates, keep_existing)
            .unwrap();

        assert_eq!(import_rows_bytes(&db), before);
        lock_connection(&db.conn)
            .execute_batch("PRAGMA query_only = OFF")
            .unwrap();
        let applied = db
            .save_imported_translations_batch_with(updates, keep_existing)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&preview.report).unwrap(),
            serde_json::to_value(&applied).unwrap(),
            "preview counts must match the following real import (keep_existing={keep_existing})"
        );
        assert_eq!(
            serde_json::to_value(&preview.report).unwrap(),
            serde_json::json!({
                "imported": if keep_existing { 4 } else { 5 },
                "unchanged": 1,
                "kept_existing": usize::from(keep_existing),
                "stale_sources": 1,
                "unknown_ids": 1,
            })
        );
        assert_eq!(preview.fill_ids, ["empty", "blank"]);
        assert_eq!(preview.unchanged_ids, ["same"]);
        assert_eq!(preview.confirmations.len(), 1);
        let confirmation = &preview.confirmations[0];
        assert_eq!(confirmation.id, "confirm");
        assert_eq!(confirmation.previous, "Hola");
        assert_eq!(confirmation.previous_status, "pending");
        assert_eq!(confirmation.new_text, "Hola");
        if keep_existing {
            assert!(preview.replacements.is_empty());
            assert_eq!(preview.kept_ids, ["different"]);
        } else {
            assert!(preview.kept_ids.is_empty());
            assert_eq!(preview.replacements.len(), 1);
            let replacement = &preview.replacements[0];
            assert_eq!(replacement.id, "different");
            assert_eq!(replacement.previous, "Hola");
            assert_eq!(replacement.previous_status, "approved");
            assert_eq!(replacement.new_text, "Bienvenido");
        }
    }
}

#[tokio::test]
async fn import_preview_refuses_duplicate_ids_and_accepts_empty_batch() {
    use crate::export::ImportedTranslation;

    let db = Database::open_in_memory().unwrap();
    db.save_entries(&[reviewed_import_entry("same", "Hello")])
        .unwrap();
    let before = import_rows_bytes(&db);
    for keep_existing in [false, true] {
        let update = ImportedTranslation {
            id: "same".into(),
            source: "Hello".into(),
            translation: "New translation".into(),
        };
        let updates = vec![update.clone(), update];
        let preview_error = db
            .preview_imported_translations(&updates, keep_existing)
            .unwrap_err();
        let save_error = db
            .save_imported_translations_batch_with(updates, keep_existing)
            .await
            .unwrap_err();
        assert!(preview_error.to_string().contains("duplicate import id"));
        assert_eq!(preview_error.to_string(), save_error.to_string());
        assert_eq!(
            serde_json::to_value(
                db.preview_imported_translations(&[], keep_existing)
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(ImportPreview::default()).unwrap()
        );
        assert_eq!(import_rows_bytes(&db), before);
    }
}

#[tokio::test]
async fn import_identical_translation_keeps_approval() {
    use crate::export::ImportedTranslation;

    for keep_existing in [false, true] {
        for status in [
            StringStatus::Approved,
            StringStatus::Reviewed,
            StringStatus::Translated,
        ] {
            let db = Database::open_in_memory().unwrap();
            let mut entry = reviewed_import_entry("same", "Hello");
            entry.status = status;
            db.save_entries(&[entry]).unwrap();
            let before = db.get_entry("same").unwrap().unwrap();
            let updates = vec![ImportedTranslation {
                id: before.id.clone(),
                source: before.source.clone(),
                translation: before.translation.clone().unwrap(),
            }];
            let report = if keep_existing {
                db.save_imported_translations_batch_with(updates, true)
                    .await
            } else {
                db.save_imported_translations_batch(updates).await
            }
            .unwrap();

            let after = db.get_entry("same").unwrap().unwrap();
            assert_eq!(after.status, before.status);
            assert_eq!(after.provider_used, before.provider_used);
            assert_eq!(after.translated_at, before.translated_at);
            assert_eq!(after.reviewed_at, before.reviewed_at);
            assert_eq!(
                serde_json::to_value(&after).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
            assert_eq!(report.imported, 1);
            assert_eq!(report.unchanged, 1);
            assert_eq!(report.kept_existing, 0);
            assert_eq!(report.stale_sources, 0);
            assert_eq!(report.unknown_ids, 0);
        }
    }
}

#[tokio::test]
async fn import_identical_translation_confirms_pending_stale_row() {
    use crate::export::ImportedTranslation;

    for keep_existing in [false, true] {
        let db = Database::open_in_memory().unwrap();
        let mut entry = make_entry("stale", "Hello");
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
        entry.provider_used = Some("original-provider".into());
        entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
        db.save_entries(&[entry]).unwrap();
        let stats = db
            .merge_entries(&[make_entry("stale", "Hello again")])
            .unwrap();
        assert_eq!(stats.stale_source_reset, 1);
        let before = db.get_entry("stale").unwrap().unwrap();
        assert_eq!(before.status, StringStatus::Pending);
        assert_eq!(before.translation.as_deref(), Some("Hola"));
        assert!(before.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));

        let report = db
            .save_imported_translations_batch_with(
                vec![ImportedTranslation {
                    id: before.id.clone(),
                    source: before.source.clone(),
                    translation: "Hola".into(),
                }],
                keep_existing,
            )
            .await
            .unwrap();

        let after = db.get_entry("stale").unwrap().unwrap();
        assert_eq!(after.translation.as_deref(), Some("Hola"));
        assert_eq!(after.status, StringStatus::Translated);
        assert_eq!(after.provider_used.as_deref(), Some("import"));
        assert!(after.translated_at.unwrap() > before.translated_at.unwrap());
        assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
        assert_eq!(report.imported, 1);
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.kept_existing, 0);
        assert_eq!(report.stale_sources, 0);
        assert_eq!(report.unknown_ids, 0);
    }
}

fn reviewed_import_entry(id: &str, source: &str) -> StringEntry {
    let mut entry = make_entry(id, source);
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Approved;
    entry.provider_used = Some("reviewed-provider".into());
    entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
    entry.reviewed_at = Some("2025-01-02T00:00:00Z".parse().unwrap());
    entry.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        serde_json::json!("preserve on no-op"),
    );
    entry.metadata.insert(
        INJECTION_SOURCE_METADATA_KEY.into(),
        serde_json::json!("日本語"),
    );
    entry
}

#[tokio::test]
async fn import_keep_existing_preserves_translated_and_fills_empty() {
    use crate::export::ImportedTranslation;

    for empty_translation in [None, Some(""), Some(" \t\r\n\u{2003}")] {
        let db = Database::open_in_memory().unwrap();
        let approved = reviewed_import_entry("approved", "Hello");
        let stale = reviewed_import_entry("stale", "Current source");
        let mut empty = make_entry("empty", "Goodbye");
        empty.translation = empty_translation.map(str::to_owned);
        db.save_entries(&[approved.clone(), empty, stale.clone()])
            .unwrap();
        let report = db
            .save_imported_translations_batch_with(
                vec![
                    ImportedTranslation {
                        id: "approved".into(),
                        source: "Hello".into(),
                        translation: "Collaborator correction".into(),
                    },
                    ImportedTranslation {
                        id: "empty".into(),
                        source: "Goodbye".into(),
                        translation: "Adiós".into(),
                    },
                    ImportedTranslation {
                        id: "stale".into(),
                        source: "Old source".into(),
                        // Source identity takes precedence even for identical text.
                        translation: "Hola".into(),
                    },
                    ImportedTranslation {
                        id: "unknown".into(),
                        source: "Unknown".into(),
                        translation: "Missing".into(),
                    },
                ],
                true,
            )
            .await
            .unwrap();

        for before in [approved, stale] {
            assert_eq!(
                serde_json::to_value(db.get_entry(&before.id).unwrap().unwrap()).unwrap(),
                serde_json::to_value(before).unwrap()
            );
        }
        let filled = db.get_entry("empty").unwrap().unwrap();
        assert_eq!(filled.translation.as_deref(), Some("Adiós"));
        assert_eq!(filled.status, StringStatus::Translated);
        assert_eq!(filled.provider_used.as_deref(), Some("import"));
        assert!(filled.translated_at.is_some());
        assert!(filled.reviewed_at.is_none());
        assert!(db.get_entry("unknown").unwrap().is_none());
        assert_eq!(report.imported, 1);
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.kept_existing, 1);
        assert_eq!(report.stale_sources, 1);
        assert_eq!(report.unknown_ids, 1);
    }
}

#[tokio::test]
async fn import_without_keep_existing_overwrites_differing_translation() {
    use crate::export::ImportedTranslation;

    let db = Database::open_in_memory().unwrap();
    let before = reviewed_import_entry("approved", "Hello");
    db.save_entries(std::slice::from_ref(&before)).unwrap();
    let report = db
        .save_imported_translations_batch(vec![ImportedTranslation {
            id: before.id.clone(),
            source: before.source.clone(),
            // Whitespace differences are not byte-identical.
            translation: "Hola ".into(),
        }])
        .await
        .unwrap();

    let after = db.get_entry(&before.id).unwrap().unwrap();
    assert_eq!(after.translation.as_deref(), Some("Hola "));
    assert_eq!(after.status, StringStatus::Translated);
    assert_eq!(after.provider_used.as_deref(), Some("import"));
    assert!(after.translated_at.unwrap() > before.translated_at.unwrap());
    assert_eq!(after.reviewed_at, before.reviewed_at);
    assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    assert_eq!(
        after.metadata.get(INJECTION_SOURCE_METADATA_KEY),
        before.metadata.get(INJECTION_SOURCE_METADATA_KEY)
    );
    assert_eq!(report.imported, 1);
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.kept_existing, 0);
    assert_eq!(report.stale_sources, 0);
    assert_eq!(report.unknown_ids, 0);
}

#[test]
fn import_report_defaults_new_counters_for_older_json() {
    let report: ImportApplyReport = serde_json::from_value(serde_json::json!({
        "imported": 1, "stale_sources": 2, "unknown_ids": 3
    }))
    .unwrap();
    assert_eq!(report.unchanged, 0);
    assert_eq!(report.kept_existing, 0);
}

#[tokio::test]
async fn imports_skip_stale_sources_without_clearing_review_or_physical_metadata() {
    use crate::export::ImportedTranslation;
    let db = Database::open_in_memory().unwrap();
    let mut stale = StringEntry::new("stale", "Current English source", "story.html".into());
    stale.translation = Some("Reviewed current translation".into());
    stale.status = StringStatus::Approved;
    stale.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        serde_json::json!("needs review"),
    );
    stale.metadata.insert(
        INJECTION_SOURCE_METADATA_KEY.into(),
        serde_json::json!("日本語"),
    );
    let current = StringEntry::new("current", "Fresh source", "story.html".into());
    db.save_entries(&[stale, current]).unwrap();
    let before = serde_json::to_value(db.get_entry("stale").unwrap().unwrap()).unwrap();
    let report = db
        .save_imported_translations_batch(vec![
            ImportedTranslation {
                id: "stale".into(),
                source: "Previous English source".into(),
                translation: "Wrong old translation".into(),
            },
            ImportedTranslation {
                id: "current".into(),
                source: "Fresh source".into(),
                translation: "Traducción actual".into(),
            },
            ImportedTranslation {
                id: "unknown".into(),
                source: "Unknown".into(),
                translation: "Unknown translation".into(),
            },
        ])
        .await
        .unwrap();
    assert_eq!(
        (report.imported, report.stale_sources, report.unknown_ids),
        (1, 1, 1)
    );
    assert_eq!(
        serde_json::to_value(db.get_entry("stale").unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        db.get_entry("current")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Traducción actual")
    );
    let duplicate = ImportedTranslation {
        id: "current".into(),
        source: "Fresh source".into(),
        translation: "Must not overwrite".into(),
    };
    assert!(db
        .save_imported_translations_batch(vec![duplicate.clone(), duplicate])
        .await
        .is_err());
    assert_eq!(
        db.get_entry("current")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Traducción actual")
    );
}

#[test]
fn test_stats_accuracy() {
    let db = Database::open_in_memory().unwrap();
    let mut t1 = make_entry("t1", "One");
    t1.status = StringStatus::Translated;
    let mut t2 = make_entry("t2", "Two");
    t2.status = StringStatus::Translated;
    db.save_entries(&[
        make_entry("p1", "A"),
        make_entry("p2", "B"),
        make_entry("p3", "C"),
        t1,
        t2,
    ])
    .unwrap();
    let stats = db.get_stats().unwrap();
    assert_eq!(stats.total, 5);
    assert_eq!(stats.pending, 3);
    assert_eq!(stats.translated, 2);
    assert_eq!(stats.reviewed, 0);
    assert_eq!(stats.approved, 0);
    assert_eq!(stats.error, 0);

    // Unknown status still counts toward total (GROUP BY must not drop it).
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO strings (id, source, file_path, status, created_at) VALUES ('x1', 'X', 'f.json', 'weird', 't')",
            [],
        )
        .unwrap();
    }
    let stats = db.get_stats().unwrap();
    assert_eq!(stats.total, 6);
    assert_eq!(stats.pending, 3);
    assert_eq!(stats.translated, 2);
}

#[test]
fn file_stats_counts_statuses_in_file_order_including_unknown() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.get_file_stats().unwrap().is_empty());
    let entries: Vec<_> = [
        (
            "b-translated",
            "b.rpy",
            StringStatus::Translated,
            Some("Hola"),
        ),
        ("b-error", "b.rpy", StringStatus::Error, None),
        (
            "a-pending",
            "a.rpy",
            StringStatus::Pending,
            Some("Stale text"),
        ),
        (
            "a-approved",
            "a.rpy",
            StringStatus::Approved,
            Some("Aprobado"),
        ),
    ]
    .into_iter()
    .map(|(id, file, status, translation)| {
        let mut entry = StringEntry::new(id, id, file.into());
        entry.status = status;
        entry.translation = translation.map(str::to_owned);
        entry
    })
    .collect();
    db.save_entries(&entries).unwrap();
    let mut expected = vec![
        FileStats {
            file_path: "a.rpy".into(),
            total: 2,
            pending: 1,
            translated: 0,
            reviewed: 0,
            approved: 1,
            error: 0,
        },
        FileStats {
            file_path: "b.rpy".into(),
            total: 2,
            pending: 0,
            translated: 1,
            reviewed: 0,
            approved: 0,
            error: 1,
        },
    ];
    assert_eq!(db.get_file_stats().unwrap(), expected);

    db.conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO strings (id, source, translation, file_path, status, created_at)
                 VALUES ('unknown', 'Unknown', 'Retained text', 'a.rpy', 'weird', 't')",
            [],
        )
        .unwrap();
    expected[0].total += 1;
    assert_eq!(db.get_file_stats().unwrap(), expected);
    let project = db.get_stats().unwrap();
    assert_eq!(project.total, 5);
    assert_eq!(project.pending, 1);
    assert_eq!(project.translated, 1);
    assert_eq!(project.reviewed, 0);
    assert_eq!(project.approved, 1);
    assert_eq!(project.error, 1);
}

#[test]
fn file_stats_counts_multiple_reviewed_rows() {
    let db = Database::open_in_memory().unwrap();
    let entries: Vec<_> = ["reviewed-1", "reviewed-2"]
        .into_iter()
        .map(|id| {
            let mut entry = StringEntry::new(id, id, "review.rpy".into());
            entry.status = StringStatus::Reviewed;
            entry
        })
        .collect();
    db.save_entries(&entries).unwrap();
    assert_eq!(
        db.get_file_stats().unwrap(),
        vec![FileStats {
            file_path: "review.rpy".into(),
            total: 2,
            reviewed: 2,
            ..FileStats::default()
        }]
    );
}

#[test]
fn test_count_entries() {
    let db = Database::open_in_memory().unwrap();
    let entries: Vec<StringEntry> = (0..4)
        .map(|i| make_entry(&format!("c{}", i), &format!("Count {}", i)))
        .collect();
    db.save_entries(&entries).unwrap();
    let count = db.count_entries(&EntryFilter::default()).unwrap();
    assert_eq!(count, 4);
}

#[test]
fn test_string_facets_are_distinct_sorted_and_project_wide() {
    let db = Database::open_in_memory().unwrap();
    let mut entries = Vec::new();
    // More rows than a typical 100-row page so facets cannot come from
    // whatever is currently on screen.
    for i in 0..120 {
        let file = match i % 3 {
            0 => "data/Map001.json",
            1 => "data/Actors.json",
            _ => "data/Map002.json",
        };
        let mut e = StringEntry::new(format!("e{i}"), format!("S{i}"), PathBuf::from(file));
        e.tags = match i % 3 {
            0 => vec!["dialogue".into(), "ui_label".into()],
            1 => vec!["ui_label".into()],
            _ => vec!["dialogue".into()],
        };
        entries.push(e);
    }
    let mut extra = StringEntry::new("zz", "Last", PathBuf::from("data/System.json"));
    extra.tags = vec!["system".into()];
    entries.push(extra);
    db.save_entries(&entries).unwrap();

    let facets = db.get_string_facets().unwrap();
    assert_eq!(
        facets.file_paths,
        vec![
            "data/Actors.json",
            "data/Map001.json",
            "data/Map002.json",
            "data/System.json",
        ]
    );
    assert_eq!(facets.tags, vec!["dialogue", "system", "ui_label"]);
}

#[test]
fn test_string_facets_empty_project() {
    let db = Database::open_in_memory().unwrap();
    let facets = db.get_string_facets().unwrap();
    assert!(facets.file_paths.is_empty());
    assert!(facets.tags.is_empty());
}

fn translated_entry(id: &str, source: &str, file: &str, translation: &str) -> StringEntry {
    let mut e = StringEntry::new(id, source, PathBuf::from(file));
    e.translation = Some(translation.into());
    e.status = StringStatus::Translated;
    e
}

#[test]
fn test_pivot_to_uses_translations_as_sources_and_skips_pending() {
    let src = Database::open_in_memory().unwrap();
    let mut done = translated_entry("a", "Hello", "data/Actors.json", "Hola");
    done.context = Some("npc".into());
    done.tags = vec!["dialogue".into()];
    done.char_limit = Some(20);
    let pending = StringEntry::new("b", "World", PathBuf::from("data/Actors.json"));
    let whitespace = translated_entry("c", "Skip me", "data/Actors.json", "   ");
    src.save_entries(&[done, pending, whitespace]).unwrap();

    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let out = dir.join("pivoted.locust.db");
    let result = src.pivot_to(&out).unwrap();
    assert_eq!(result.entries, 1);
    assert_eq!(result.database_path, out.to_string_lossy());

    let new_db = Database::open(&out).unwrap();
    let entries = new_db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "a");
    assert_eq!(entries[0].source, "Hola");
    assert!(entries[0].translation.is_none());
    assert_eq!(entries[0].status, StringStatus::Pending);
    assert_eq!(entries[0].context.as_deref(), Some("npc"));
    assert_eq!(entries[0].tags, vec!["dialogue".to_string()]);
    assert_eq!(entries[0].char_limit, Some(20));
    assert_eq!(entries[0].injection_source().unwrap(), "Hello");

    let original = src.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(original.len(), 3);
    let hello = original.iter().find(|e| e.id == "a").unwrap();
    assert_eq!(hello.source, "Hello");
    assert_eq!(hello.translation.as_deref(), Some("Hola"));
    assert_eq!(hello.status, StringStatus::Translated);
    let world = original.iter().find(|e| e.id == "b").unwrap();
    assert_eq!(world.source, "World");
    assert!(world.translation.is_none());
}

fn assert_complete_pivot_refused(entries: &[StringEntry], reason: &str) {
    let src = Database::open_in_memory().unwrap();
    src.save_entries(entries).unwrap();
    let before = serde_json::to_value(src.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let dir = recording_tempdir();
    let error = src
        .pivot_to_complete(&dir.path().join("complete.db"))
        .expect_err(reason);
    assert!(error.to_string().contains(reason), "{error}");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "no DB or sidecars on refusal"
    );
    let after = serde_json::to_value(src.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    assert_eq!(after, before, "refusal must not change source rows");
}

#[test]
fn pivot_complete_refuses_missing_or_empty_translation_in_two_row_inventory() {
    for translation in [None, Some("")] {
        let done = translated_entry("a", "日本語", "script.rpy", "English");
        let mut missing = translated_entry("z", "別の行", "script.rpy", "unused");
        missing.translation = translation.map(str::to_owned);
        assert_complete_pivot_refused(&[done, missing], "missing or blank translation");
    }
}

#[test]
fn pivot_complete_refuses_unicode_whitespace_translation() {
    for translation in ["   ", "\t\n", "\u{a0}\u{2003}\u{3000}"] {
        assert_complete_pivot_refused(
            &[
                translated_entry("a", "日本語", "script.rpy", "English"),
                translated_entry("z", "別の行", "script.rpy", translation),
            ],
            "missing or blank translation",
        );
    }
}

#[test]
fn pivot_complete_refuses_not_ready_nonempty_retained_translation() {
    for status in [StringStatus::Pending, StringStatus::Error] {
        let mut retained = translated_entry("z", "別の行", "script.rpy", "Retained English");
        retained.status = status;
        assert_complete_pivot_refused(
            &[
                translated_entry("a", "日本語", "script.rpy", "English"),
                retained,
            ],
            "not ready",
        );
    }
}

#[test]
fn pivot_complete_uses_one_snapshot_and_preserves_every_ready_row() {
    let src = Database::open_in_memory().unwrap();
    let mut name = translated_entry(
        "a",
        r"\n<錬金術師>こんにちは",
        "data/Map001.json",
        r"\n<Alchemist>Hello",
    );
    name.context = Some("speaker".into());
    name.tags = vec!["dialogue".into()];
    name.metadata
        .insert("format_id".into(), serde_json::json!("rpgmaker-mv"));
    let mut reviewed = translated_entry("b", "こんにちは [name]", "script.rpy", "Hello [name]");
    reviewed.status = StringStatus::Reviewed;
    let mut approved = translated_entry("c", "開始", "script.rpy", "Start");
    approved.status = StringStatus::Approved;
    src.save_entries(&[approved, name, reviewed]).unwrap();
    src.set_project_metadata("source_lang", &serde_json::json!("ja"))
        .unwrap();
    let before = serde_json::to_value(src.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let dir = recording_tempdir();
    let output = dir.path().join("complete.db");
    STRINGS_QUERIES.with(|count| count.set(Some(0)));
    let result = src.pivot_to_complete(&output);
    let queries = STRINGS_QUERIES.with(|count| count.replace(None));
    assert_eq!(result.unwrap().entries, 3);
    assert_eq!(
        queries,
        Some(1),
        "validate and pivot one row snapshot, not count then reload"
    );
    let pivot = Database::open(&output).unwrap();
    let rows = pivot.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
    for row in &rows {
        let original = src.get_entry(&row.id).unwrap().unwrap();
        assert_eq!(Some(row.source.as_str()), original.translation.as_deref());
        assert_eq!(row.injection_source().unwrap(), original.source);
        assert_eq!(row.status, StringStatus::Pending);
        assert!(row.translation.is_none());
        assert_eq!(row.context, original.context);
        assert_eq!(row.tags, original.tags);
    }
    assert_eq!(rows[0].source, r"\n<Alchemist>Hello");
    let (provider_text, _) = crate::placeholder::PlaceholderProcessor::extract_with_profile(
        &rows[0].source,
        rows[0].placeholder_profile(),
    );
    assert!(
        provider_text.contains("<Alchemist>"),
        "speaker title must remain translatable"
    );
    assert_eq!(
        pivot.get_project_metadata("source_lang").unwrap(),
        Some(serde_json::json!("ja"))
    );
    assert_eq!(
        serde_json::to_value(src.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
        before
    );
}

#[test]
fn pivot_complete_keeps_control_and_stale_preflight_before_output_creation() {
    let missing_control = translated_entry("z", "日本語 [name]", "script.rpy", "English");
    let mut physical_control = translated_entry("z", "English", "script.rpy", "Spanish");
    physical_control.metadata.insert(
        INJECTION_SOURCE_METADATA_KEY.into(),
        serde_json::json!("日本語 [name]"),
    );
    let mut stale = translated_entry("z", "新しい原文", "script.rpy", "Old English");
    stale.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        serde_json::json!({
            "version": 1,
            "translated_source_sha256": "0".repeat(64),
            "current_source_sha256": stale.source_hash(),
        }),
    );
    for (invalid, reason) in [
        (missing_control, "changed protected controls"),
        (physical_control, "changed protected controls"),
        (stale, "stale translation"),
    ] {
        assert_complete_pivot_refused(
            &[
                translated_entry("a", "日本語", "script.rpy", "English"),
                invalid,
            ],
            reason,
        );
    }
}

#[test]
fn pivot_complete_refuses_empty_database_and_preserves_existing_output() {
    assert_complete_pivot_refused(&[], "no translated entries");
    let src = Database::open_in_memory().unwrap();
    src.save_entries(&[translated_entry("a", "日本語", "script.rpy", "English")])
        .unwrap();
    let dir = recording_tempdir();
    let output = dir.path().join("existing.db");
    std::fs::write(&output, b"existing output").unwrap();
    let error = src.pivot_to_complete(&output).unwrap_err();
    assert!(
        error.to_string().contains("refusing to overwrite"),
        "{error}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn repeated_pivot_keeps_original_source_and_binary_capacity() {
    let src = Database::open_in_memory().unwrap();
    let mut japanese = translated_entry("slot", "日本語文", "resources.assets", "English");
    japanese
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    src.save_entries(&[japanese]).unwrap();

    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let first_path = dir.join("en.locust.db");
    src.pivot_to(&first_path).unwrap();
    let first = Database::open(&first_path).unwrap();
    let first_entry = first.get_entry("slot").unwrap().unwrap();
    assert_eq!(first_entry.source, "English");
    assert_eq!(first_entry.injection_source().unwrap(), "日本語文");
    assert_eq!(
        first_entry.injection_capacity().unwrap(),
        Some(("utf8", 12))
    );
    first
        .save_translation("slot", "1234567890", "manual")
        .await
        .unwrap();

    let second_path = dir.join("es.locust.db");
    first.pivot_to(&second_path).unwrap();
    let second = Database::open(&second_path)
        .unwrap()
        .get_entry("slot")
        .unwrap()
        .unwrap();
    assert_eq!(second.source, "1234567890");
    assert_eq!(second.injection_source().unwrap(), "日本語文");
    assert_eq!(second.injection_capacity().unwrap(), Some(("utf8", 12)));
}

#[test]
fn pivot_preserves_grouped_textasset_original_and_does_not_invent_cell_capacity() {
    let src = Database::open_in_memory().unwrap();
    let original = "Menu.A: 日本語\nMenu.B: 行く\n";
    let mut a = translated_entry("a", "日本語", "resources.assets", "English A");
    let mut b = translated_entry("b", "行く", "resources.assets", "Go");
    for (entry, key, index) in [(&mut a, "Menu.A", 0usize), (&mut b, "Menu.B", 1usize)] {
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        entry
            .metadata
            .insert("loc_key".into(), serde_json::json!(key));
        entry
            .metadata
            .insert("line_index".into(), serde_json::json!(index));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
    }
    let mut members = vec![a, b];
    crate::textasset_group::attach_to_entries(
        &mut members,
        crate::textasset_group::GroupKind::LocLine,
        original,
        original.len(),
        "g-pivot",
    );
    src.save_entries(&members).unwrap();

    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let out = dir.join("pivot-group.locust.db");
    src.pivot_to(&out).unwrap();
    let pivoted = Database::open(&out).unwrap();
    let first = pivoted.get_entry("a").unwrap().unwrap();
    assert_eq!(first.source, "English A");
    assert_eq!(first.injection_source().unwrap(), "日本語");
    assert!(first.injection_capacity().unwrap().is_none());
    assert_eq!(
        crate::textasset_group::original_textasset(&first),
        Some(original)
    );
    assert_eq!(
        first
            .metadata
            .get(crate::textasset_group::GROUP_CAPACITY_KEY)
            .and_then(|v| v.as_u64()),
        Some(original.len() as u64)
    );
    assert_eq!(crate::validation::binary_slot_budget(&first).unwrap(), None);
}

#[tokio::test]
async fn pivot_merge_preserves_semantic_source_and_rejects_changed_baseline_atomically() {
    let src = Database::open_in_memory().unwrap();
    src.save_entries(&[
        translated_entry("a", "日本語A", "old-a", "English A"),
        translated_entry("b", "日本語B", "old-b", "English B"),
    ])
    .unwrap();
    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let pivot_path = dir.join("merge.locust.db");
    src.pivot_to(&pivot_path).unwrap();
    let pivot = Database::open(&pivot_path).unwrap();
    pivot
        .save_translation("a", "Español A", "manual")
        .await
        .unwrap();

    let mut same_a = make_entry("a", "日本語A");
    same_a.file_path = PathBuf::from("new-a");
    same_a
        .metadata
        .insert("fresh_locator".into(), serde_json::json!(7));
    let mut same_b = make_entry("b", "日本語B");
    same_b.file_path = PathBuf::from("new-b");
    pivot.merge_entries(&[same_a, same_b]).unwrap();
    let kept = pivot.get_entry("a").unwrap().unwrap();
    assert_eq!(kept.source, "English A");
    assert_eq!(kept.translation.as_deref(), Some("Español A"));
    assert_eq!(kept.injection_source().unwrap(), "日本語A");
    assert_eq!(kept.file_path, PathBuf::from("new-a"));
    assert_eq!(
        kept.metadata.get("fresh_locator"),
        Some(&serde_json::json!(7))
    );

    let before_a = pivot.get_entry("a").unwrap().unwrap();
    let before_b = pivot.get_entry("b").unwrap().unwrap();
    let mut would_update_a = make_entry("a", "日本語A");
    would_update_a.file_path = PathBuf::from("must-not-write");
    let changed_b = make_entry("b", "different baseline");
    let error = pivot
        .merge_entries(&[would_update_a, changed_b])
        .expect_err("changed physical baseline must fail before writes");
    assert!(error.to_string().contains("source_changed"));
    assert_eq!(
        pivot.get_entry("a").unwrap().unwrap().file_path,
        before_a.file_path
    );
    assert_eq!(
        pivot.get_entry("b").unwrap().unwrap().file_path,
        before_b.file_path
    );
    // Some formats encode source text in IDs. A changed baseline may
    // therefore look like a missing row, not an overlapping changed source.
    let error = pivot
        .merge_entries(&[make_entry("a", "日本語A")])
        .expect_err("a missing physical slot must not delete a pivot translation");
    assert!(error.to_string().contains("missing entries"));
    assert_eq!(pivot.get_entry("a").unwrap().unwrap().source, "English A");
    assert!(pivot.get_entry("b").unwrap().is_some());
}

/// Pin: pivot's source query must not materialize the pending bulk that
/// `get_entries` still returns (negative: equal lengths would mean the
/// SQL filter is gone and large projects pay O(n) for mostly-empty rows).
#[test]
fn test_entries_with_nonempty_translation_skips_pending_bulk() {
    let src = Database::open_in_memory().unwrap();
    let mut bulk: Vec<StringEntry> = (0..200)
        .map(|i| {
            StringEntry::new(
                format!("p{i}"),
                format!("Pending {i}"),
                PathBuf::from("f.json"),
            )
        })
        .collect();
    bulk.push(translated_entry("done", "Hello", "f.json", "Hola"));
    bulk.push(translated_entry("ws", "Whitespace", "f.json", "   "));
    src.save_entries(&bulk).unwrap();

    let pivoted_src = src.entries_with_nonempty_translation().unwrap();
    assert_eq!(pivoted_src.len(), 1);
    assert_eq!(pivoted_src[0].id, "done");
    assert_eq!(
        src.get_entries(&EntryFilter::default()).unwrap().len(),
        202,
        "full table scan still sees every row; pivot query must stay narrower"
    );
}

#[test]
fn test_pivot_to_refuses_existing_output_path() {
    let src = Database::open_in_memory().unwrap();
    src.save_entries(&[translated_entry("a", "Hello", "f.json", "Hola")])
        .unwrap();

    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let out = dir.join("exists.locust.db");
    std::fs::write(&out, b"nope").unwrap();
    let err = src.pivot_to(&out).expect_err("must refuse overwrite");
    let msg = err.to_string().to_lowercase();
    assert!(msg.contains("exist") || msg.contains("overwrite"), "{msg}");
    assert_eq!(std::fs::read(&out).unwrap(), b"nope");
}

#[test]
fn test_pivot_to_errors_when_nothing_is_translated() {
    let src = Database::open_in_memory().unwrap();
    src.save_entries(&[StringEntry::new("b", "World", PathBuf::from("f.json"))])
        .unwrap();
    let dir_guard = recording_tempdir();
    let dir = dir_guard.path().to_path_buf();
    let out = dir.join("empty-pivot.locust.db");
    let err = src.pivot_to(&out).expect_err("must refuse empty pivot");
    assert!(
        err.to_string().to_lowercase().contains("no translated"),
        "{err}"
    );
    assert!(!out.exists(), "must not create an empty output db");
}

#[test]
fn control_repro_pivot_refuses_missing_controls_before_creating_output() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("source.db")).unwrap();
    let mut entry = StringEntry::new("line", "JA {name}", PathBuf::from("script.rpy"));
    entry.translation = Some("Hello".into());
    db.save_entries(&[entry]).unwrap();
    let output = temp.path().join("pivot.db");
    assert!(
        db.pivot_to(&output).is_err(),
        "invalid English must not become a pivot source"
    );
    assert!(!output.exists());
}

#[tokio::test]
async fn control_manual_and_imported_results_remain_reportable_and_block_pivot() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("source.db")).unwrap();
    let entry = StringEntry::new("line", r"JA \V[1]", PathBuf::from("data.json"));
    db.save_entries(&[entry]).unwrap();
    db.save_translation("line", "Hello", "manual")
        .await
        .unwrap();
    let saved = db.get_entries(&EntryFilter::default()).unwrap().remove(0);
    assert!(!crate::validation::Validator::validate_entry(&saved).is_empty());
    assert!(db.pivot_to(&temp.path().join("invalid.db")).is_err());
    // Batch import shares the same report/gate, while allowing stored draft
    // work to be repaired explicitly rather than deleting the user's text.
    db.save_translations_batch(vec![("line".into(), r"Hello \V[1]".into())], "import")
        .await
        .unwrap();
    let es = temp.path().join("es.db");
    let fr = temp.path().join("fr.db");
    db.pivot_to(&es).unwrap();
    db.pivot_to(&fr).unwrap();
    let es = Database::open(&es).unwrap();
    let fr = Database::open(&fr).unwrap();
    es.save_translation("line", r"Hola \V[1]", "manual")
        .await
        .unwrap();
    let saved = es.get_entries(&EntryFilter::default()).unwrap().remove(0);
    assert!(saved.require_preserved_translation_controls().is_ok());
    assert_eq!(saved.source, r"Hello \V[1]");
    assert_eq!(saved.injection_source().unwrap(), r"JA \V[1]");
    assert!(fr.get_entries(&EntryFilter::default()).unwrap()[0]
        .translation
        .is_none());
}
