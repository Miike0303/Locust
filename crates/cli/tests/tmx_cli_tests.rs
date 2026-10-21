use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};

use locust_core::database::{Database, EntryFilter};
use locust_core::export::{export_tmx, import_tmx};
use locust_core::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};
use serde_json::{json, Value};

struct Fixture {
    root: tempfile::TempDir,
    project: PathBuf,
    input: PathBuf,
}

fn row(id: &str, source: &str, target: Option<&str>) -> StringEntry {
    let mut row = StringEntry::new(id, source, "story.rpy".into());
    row.translation = target.map(str::to_owned);
    row.context = Some("Synthetic context".into());
    row.tags = vec!["dialogue".into()];
    row.char_limit = Some(200);
    row.metadata.insert("retained".into(), json!(42));
    row.created_at = "2025-01-01T00:00:00Z".parse().unwrap();
    row
}

fn approved(id: &str, source: &str, target: &str) -> StringEntry {
    let mut row = row(id, source, Some(target));
    row.status = StringStatus::Approved;
    row.provider_used = Some("local-review".into());
    row.translated_at = Some("2025-01-02T00:00:00Z".parse().unwrap());
    row.reviewed_at = Some("2025-01-03T00:00:00Z".parse().unwrap());
    row
}

impl Fixture {
    fn new(rows: &[StringEntry]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project.db");
        Database::open(&project)
            .unwrap()
            .save_entries(rows)
            .unwrap();
        let input = root.path().join("catalog.tmx");
        Self {
            root,
            project,
            input,
        }
    }

    fn command(&self, action: &str, format: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_locust"));
        command
            .env("LOCUST_DATA_DIR", self.root.path().join("profile"))
            .env("LOCUST_BACKUP_ROOT", self.root.path().join("backups"))
            .arg(action)
            .arg(&self.project)
            .args(["-f", format, "-l", "es"]);
        command
    }

    fn import(&self, extra: &[&str]) -> Output {
        self.command("import", "tmx")
            .args(["--source-lang", "en", "-i"])
            .arg(&self.input)
            .args(extra)
            .output()
            .unwrap()
    }

    fn catalog(&self, rows: &[StringEntry]) {
        std::fs::write(&self.input, export_tmx(rows, "en", "es").unwrap()).unwrap();
    }

    fn snapshot(&self) -> Value {
        let db = Database::open(&self.project).unwrap();
        json!({
            "rows": db.get_entries(&EntryFilter::default()).unwrap(),
            "stats": db.get_stats().unwrap(),
            "files": db.get_file_stats().unwrap(),
            "count": db.count_entries(&EntryFilter::default()).unwrap(),
            "translated_count": db.count_translated_entries().unwrap(),
            "memory_count": db.memory_count().unwrap(),
        })
    }
}

fn result(output: Output, exit: i32, stdout: &str, stderr: &str) {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), stdout);
    assert_eq!(String::from_utf8(output.stderr).unwrap(), stderr);
}

fn imported(f: &Fixture, count: usize) -> String {
    format!("Imported {count} translations from {}\n", f.input.display())
}

fn profile_error(message: &str) -> String {
    format!("Error: parse error in tmx: TMX profile error: {message}\n")
}

// Build the complete expected public snapshot independently of the importer.
fn expected_snapshot(rows: &[StringEntry]) -> Value {
    let mut rows = rows.to_vec();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    let counts = |rows: &[StringEntry]| {
        let mut counts = json!({"total": rows.len(), "pending": 0, "translated": 0,
            "reviewed": 0, "approved": 0, "error": 0});
        for row in rows {
            let key = row.status.to_string();
            counts[&key] = json!(counts[&key].as_u64().unwrap() + 1);
        }
        counts
    };
    let mut stats = counts(&rows);
    stats["total_cost_usd"] = json!(0.0);
    let mut files: BTreeMap<PathBuf, Vec<StringEntry>> = BTreeMap::new();
    for row in &rows {
        files
            .entry(row.file_path.clone())
            .or_default()
            .push(row.clone());
    }
    let files: Vec<Value> = files
        .into_iter()
        .map(|(path, rows)| {
            let mut file = counts(&rows);
            file["file_path"] = json!(path);
            file
        })
        .collect();
    let translated_count = rows
        .iter()
        .filter(|row| {
            matches!(
                row.status,
                StringStatus::Translated | StringStatus::Reviewed | StringStatus::Approved
            ) && row
                .translation
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
        })
        .count();
    json!({"rows": rows, "stats": stats, "files": files, "count": rows.len(),
        "translated_count": translated_count, "memory_count": 0})
}

#[test]
fn tmx_public_cli_roundtrip_preserves_exact_identity_and_approved_noop() {
    let rows = [approved(
        "id \t\r\n界",
        "  Hello\r\n[name] & <x>  ",
        " Hola\r\n界 ",
    )];
    let f = Fixture::new(&rows);
    let before = f.snapshot();
    assert_eq!(before, expected_snapshot(&rows));
    result(
        f.command("export", "tmx")
            .args(["--source-lang", "en", "-o"])
            .arg(&f.input)
            .output()
            .unwrap(),
        0,
        &format!("Exported 1 entries to {} (en→es)\n", f.input.display()),
        "",
    );
    assert_eq!(
        std::fs::read_to_string(&f.input).unwrap(),
        export_tmx(&rows, "en", "es").unwrap()
    );
    for _ in 0..2 {
        result(
            f.import(&[]),
            0,
            &(imported(&f, 1) + "Unchanged 1 (already identical)\n"),
            "",
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn rejects_late_units_languages_and_ambiguous_xml_without_any_saved_change() {
    let f = Fixture::new(&[approved("a", "Hello", "Saved")]);
    let valid = export_tmx(&[row("a", "Hello", Some("Changed"))], "en", "es").unwrap();
    let before = f.snapshot();
    let cases = [
        (
            valid.replace("</body>", "<tu tuid=\"locust-v1-62\"></tu></body>"),
            "unit must contain both language variants",
        ),
        (valid.replace("</tmx>", ""), "empty or incomplete document"),
        (
            format!("{valid}<tmx version=\"1.4\"></tmx>"),
            "multiple document roots",
        ),
        (
            valid.replace("srclang=\"en\"", "srclang=\"ja\""),
            "header source language mismatch",
        ),
        (
            valid.replace("xml:lang=\"es\"", "xml:lang=\"es-ES\""),
            "unexpected variant language",
        ),
        (
            valid.replace("<tuv xml:lang=\"es\"><seg>Changed</seg></tuv>", ""),
            "unit must contain both language variants",
        ),
        (
            valid.replace("xml:lang=\"es\"", "xml:lang=\"en\""),
            "duplicate language variant",
        ),
        (
            valid.replace("<seg>Changed</seg>", "<seg><b>Changed</b></seg>"),
            "unsupported element, namespace, inline markup or nesting",
        ),
        (
            valid.replace("<body>", "<!DOCTYPE body><body>"),
            "DTD and processing instructions are unsupported",
        ),
        (
            valid.replace("locust-v1-61", "locust-v1-"),
            "empty or duplicate decoded project id",
        ),
        (
            valid.replace("</body>", "<tu tuid=\"locust-v1-61\"></tu></body>"),
            "empty or duplicate decoded project id",
        ),
    ];
    for (document, error) in cases {
        std::fs::write(&f.input, document).unwrap();
        for args in [vec![], vec!["--dry-run"], vec!["--keep-existing"]] {
            result(f.import(&args), 1, "", &profile_error(error));
            assert_eq!(f.snapshot(), before, "{error}");
        }
    }
}

#[test]
fn explicit_languages_are_required_concrete_distinct_and_not_inferred() {
    let f = Fixture::new(&[row("a", "Hello", None)]);
    f.catalog(&[row("a", "Hello", Some("Hola"))]);
    let before = f.snapshot();
    for action in ["import", "export"] {
        let option = if action == "import" { "-i" } else { "-o" };
        let path = f.root.path().join(format!("{action}.tmx"));
        std::fs::copy(&f.input, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut command = f.command(action, "tmx");
        command.arg(option).arg(&path);
        if action == "export" {
            command.arg("--overwrite");
        }
        result(
            command.output().unwrap(),
            1,
            "",
            "Error: --source-lang is required for TMX\n",
        );
        for (source, target, message) in [
            ("und", "es", "expected a concrete ASCII language tag"),
            ("en", "*all*", "expected a concrete ASCII language tag"),
            ("en", "EN", "source and target languages must differ"),
            ("en_US", "es", "expected a concrete ASCII language tag"),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_locust"));
            command
                .env("LOCUST_DATA_DIR", f.root.path().join("profile"))
                .env("LOCUST_BACKUP_ROOT", f.root.path().join("backups"))
                .arg(action)
                .arg(&f.project)
                .args(["-f", "tmx", "-l", target, "--source-lang", source, option])
                .arg(&path);
            if action == "export" {
                command.arg("--overwrite");
            }
            result(command.output().unwrap(), 1, "", &profile_error(message));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(f.snapshot(), before);
        }
    }
    // Case-insensitive exact tags are allowed, without guessing regions.
    result(
        f.command("import", "tmx")
            .args(["--source-lang", "EN", "-i"])
            .arg(&f.input)
            .args(["--dry-run"])
            .output()
            .unwrap(),
        0,
        "Would replace 0 translation(s):\nWould fill 1 empty row(s)\nDry run: nothing was saved.\n",
        "",
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn rejects_invalid_utf8_and_bounded_file_read_before_import() {
    let f = Fixture::new(&[approved("a", "Hello", "Saved")]);
    let before = f.snapshot();
    std::fs::write(&f.input, [0xff]).unwrap();
    result(
        f.import(&[]),
        1,
        "",
        "Error: invalid utf-8 sequence of 1 bytes from index 0\n",
    );
    assert_eq!(f.snapshot(), before);
    // Sparse input: the CLI must read at most 32 MiB + 1, not allocate this file.
    std::fs::File::create(&f.input)
        .unwrap()
        .set_len(512 * 1024 * 1024)
        .unwrap();
    result(f.import(&[]), 1, "", "Error: TMX input exceeds 32 MiB\n");
    assert_eq!(f.snapshot(), before);
    let mut document = export_tmx(&[], "en", "es").unwrap();
    document.push_str(&" ".repeat(32 * 1024 * 1024 - document.len()));
    std::fs::write(&f.input, &document).unwrap();
    result(f.import(&[]), 0, &imported(&f, 0), "");
    document.push(' ');
    std::fs::write(&f.input, document).unwrap();
    result(f.import(&[]), 1, "", "Error: TMX input exceeds 32 MiB\n");
    assert_eq!(f.snapshot(), before);
}

#[test]
fn rejects_decoded_segment_identity_and_unit_limits_without_partial_import() {
    let f = Fixture::new(&[approved("a", "Hello", "Saved")]);
    let before = f.snapshot();
    let valid = export_tmx(&[row("a", "Hello", Some("Changed"))], "en", "es").unwrap();
    let cases = [
        (
            valid.replace(
                "<seg>Changed</seg>",
                &format!("<seg>{}</seg>", "x".repeat(1024 * 1024 + 1)),
            ),
            "decoded segment exceeds 1 MiB",
        ),
        (
            valid.replace(
                "locust-v1-61",
                &format!("locust-v1-{}", "61".repeat(16 * 1024 + 1)),
            ),
            "encoded id exceeds limit",
        ),
        (
            valid.replace(
                "</body>",
                &(String::from("<tu tuid=\"locust-v1-62\">")
                    + &"<tuv xml:lang=\"en\"><seg>x</seg></tuv>".repeat(2)
                    + "</tu></body>"),
            ),
            "duplicate language variant",
        ),
    ];
    for (document, message) in cases {
        std::fs::write(&f.input, document).unwrap();
        result(f.import(&[]), 1, "", &profile_error(message));
        assert_eq!(f.snapshot(), before);
    }
    let header = export_tmx(&[], "en", "es").unwrap();
    let mut units = String::new();
    for i in 0..100_001u32 {
        // Unique reversible UTF-8 IDs, without a codec call that rejects early.
        let id: String = format!("{i}").bytes().map(|b| format!("{b:02x}")).collect();
        units.push_str(&format!("<tu tuid=\"locust-v1-{id}\"><tuv xml:lang=\"en\"><seg>x</seg></tuv><tuv xml:lang=\"es\"><seg>y</seg></tuv></tu>"));
    }
    std::fs::write(&f.input, header.replace("</body>", &(units + "</body>"))).unwrap();
    result(
        f.import(&[]),
        1,
        "",
        &profile_error("too many translation units"),
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn stale_empty_unknown_and_exact_source_mismatch_are_counted_not_confirmed() {
    let mut stale = approved("stale", "Current", "Anterior");
    stale.status = StringStatus::Pending;
    stale
        .metadata
        .insert(STALE_TRANSLATION_METADATA_KEY.into(), json!(true));
    let rows = [
        stale.clone(),
        approved("empty", "Empty", "Saved"),
        approved("mismatch", "Hello ", "Saved"),
    ];
    let f = Fixture::new(&rows);
    let before = f.snapshot();
    stale.translation = Some("Edited but unreviewed".into());
    f.catalog(&[
        stale,
        row("empty", "Empty", None),
        row("mismatch", "Hello", Some("Wrong")),
        row("unknown", "Unknown", Some("Missing")),
    ]);
    let skip = "Skipped 4: 1 outdated source(s), 1 unknown id(s), 2 empty/missing/fuzzy entries\n";
    result(f.import(&[]), 0, &(imported(&f, 0) + skip), "");
    result(
        f.import(&["--dry-run"]),
        0,
        &(String::from("Would replace 0 translation(s):\nWould fill 0 empty row(s)\n")
            + skip
            + "Dry run: nothing was saved.\n"),
        "",
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn preview_keep_existing_confirmation_and_reimport_match_full_public_snapshots() {
    for keep in [false, true] {
        let mut confirm = approved("confirm", "Current", "Anterior");
        confirm.status = StringStatus::Pending;
        confirm
            .metadata
            .insert(STALE_TRANSLATION_METADATA_KEY.into(), json!(true));
        let mut rows = vec![
            row("blank", "Blank", Some(" \t")),
            confirm,
            approved("different", "Different", "Local"),
            row("empty", "Empty", None),
            approved("same", "Same", "Igual"),
        ];
        let f = Fixture::new(&rows);
        let before = f.snapshot();
        assert_eq!(before, expected_snapshot(&rows));
        f.catalog(&[
            row("blank", "Blank", Some("Blanco")),
            row("confirm", "Current", Some("Anterior")),
            row("different", "Different", Some("Nuevo")),
            row("empty", "Empty", Some("Vacío")),
            row("same", "Same", Some("Igual")),
        ]);
        let extra = if keep {
            vec!["--keep-existing"]
        } else {
            vec![]
        };
        let mut preview_args = extra.clone();
        preview_args.push("--dry-run");
        let replacement = if keep {
            ""
        } else {
            "  different: \"Local\" (approved) -> \"Nuevo\"\n"
        };
        let kept_preview = if keep {
            "Kept 1 (--keep-existing)\n"
        } else {
            ""
        };
        result(f.import(&preview_args), 0, &format!("Would replace {} translation(s):\n{replacement}  confirm: \"Anterior\" (pending) -> \"Anterior\"\nWould fill 2 empty row(s)\nUnchanged 1 (already identical)\n{kept_preview}Dry run: nothing was saved.\n", if keep {1} else {2}), "");
        assert_eq!(f.snapshot(), before);
        let start = chrono::Utc::now();
        let kept = if keep {
            "Kept 1 existing translation(s) (--keep-existing)\n"
        } else {
            ""
        };
        result(
            f.import(&extra),
            0,
            &(imported(&f, if keep { 4 } else { 5 }) + "Unchanged 1 (already identical)\n" + kept),
            "",
        );
        let end = chrono::Utc::now();
        let after = f.snapshot();
        for entry in &mut rows {
            let target = match entry.id.as_str() {
                "blank" => "Blanco",
                "confirm" => "Anterior",
                "empty" => "Vacío",
                "different" if !keep => "Nuevo",
                _ => continue,
            };
            let actual = after["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == entry.id)
                .unwrap();
            let time: chrono::DateTime<chrono::Utc> =
                actual["translated_at"].as_str().unwrap().parse().unwrap();
            assert!(time >= start && time <= end);
            entry.translation = Some(target.into());
            entry.status = StringStatus::Translated;
            entry.provider_used = Some("import".into());
            entry.translated_at = Some(time);
            entry.metadata.remove(STALE_TRANSLATION_METADATA_KEY);
        }
        assert_eq!(after, expected_snapshot(&rows));
        result(
            f.import(&extra),
            0,
            &(imported(&f, if keep { 4 } else { 5 })
                + &format!(
                    "Unchanged {} (already identical)\n{kept}",
                    if keep { 4 } else { 5 }
                )),
            "",
        );
        assert_eq!(f.snapshot(), after);
    }
}

#[test]
fn tmx_export_filters_overwrite_and_database_sidecar_guards_are_shared() {
    let mut other = row("b", "Other", None);
    other.file_path = "other.rpy".into();
    let rows = [approved("a", "Hello", "Hola"), other];
    let f = Fixture::new(&rows);
    let before = f.snapshot();
    let run = |path: &std::path::Path, extra: &[&str]| {
        f.command("export", "tmx")
            .args(["--source-lang", "en", "-o"])
            .arg(path)
            .args(extra)
            .output()
            .unwrap()
    };
    result(
        run(
            &f.input,
            &[
                "--file",
                "story.rpy",
                "--status",
                "approved",
                "--status",
                "approved",
            ],
        ),
        0,
        &format!("Exported 1 entries to {} (en→es)\n", f.input.display()),
        "",
    );
    let bytes = std::fs::read(&f.input).unwrap();
    assert_eq!(
        import_tmx(std::str::from_utf8(&bytes).unwrap(), "en", "es")
            .unwrap()
            .len(),
        1
    );
    result(
        run(&f.input, &[]),
        1,
        "",
        &format!(
            "Error: {} already exists; pass --overwrite to replace it\n",
            f.input.display()
        ),
    );
    for (args, error) in [
        (vec!["--overwrite", "--file", "story.rpy", "--status", "pending"], "no entries to export"),
        (vec!["--overwrite", "--file", "missing.rpy"], "no entries to export for file 'missing.rpy'; known files: other.rpy, story.rpy"),
        (vec!["--overwrite", "--status", "bogus"], "invalid export status 'bogus'; valid values: pending, translated, reviewed, approved, error"),
    ] {
        result(run(&f.input, &args), 1, "", &format!("Error: {error}\n"));
        assert_eq!(std::fs::read(&f.input).unwrap(), bytes);
    }
    result(
        run(&f.input, &["--overwrite"]),
        0,
        &format!("Exported 2 entries to {} (en→es)\n", f.input.display()),
        "",
    );
    result(
        run(&f.project, &["--overwrite"]),
        1,
        "",
        "Error: the export destination is the project database itself; choose a different file\n",
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = PathBuf::from(format!("{}{suffix}", f.project.display()));
        result(run(&path, &["--overwrite"]), 1, "", &format!("Error: the export destination is the project database's SQLite {suffix} sidecar; choose a different file\n"));
    }
    let alias = f.root.path().join("database-alias.tmx");
    std::fs::hard_link(&f.project, &alias).unwrap();
    result(
        run(&alias, &["--overwrite"]),
        1,
        "",
        "Error: the export destination is the project database itself; choose a different file\n",
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn export_limits_fail_before_replacing_catalog_and_empty_selection_creates_nothing() {
    let rows = [row("a", &"x".repeat(1024 * 1024 + 1), Some("Hola"))];
    let f = Fixture::new(&rows);
    let before = f.snapshot();
    std::fs::write(&f.input, b"Retained catalog").unwrap();
    result(
        f.command("export", "tmx")
            .args(["--source-lang", "en", "-o"])
            .arg(&f.input)
            .arg("--overwrite")
            .output()
            .unwrap(),
        1,
        "",
        &profile_error("decoded field exceeds byte limit"),
    );
    assert_eq!(std::fs::read(&f.input).unwrap(), b"Retained catalog");
    assert_eq!(f.snapshot(), before);
    let empty = Fixture::new(&[]);
    let before = empty.snapshot();
    result(
        empty
            .command("export", "tmx")
            .args(["--source-lang", "en", "-o"])
            .arg(&empty.input)
            .output()
            .unwrap(),
        1,
        "",
        "Error: no entries to export\n",
    );
    assert!(!empty.input.exists());
    assert_eq!(empty.snapshot(), before);
}

#[test]
fn source_lang_is_explicitly_ignored_for_existing_formats_in_shared_handlers() {
    let rows = [approved("a", "Hello", "Hola")];
    let f = Fixture::new(&rows);
    let before = f.snapshot();
    for format in ["po", "xliff"] {
        let output = f.root.path().join(format!("catalog.{format}"));
        let run = |extra: &[&str]| {
            f.command("export", format)
                .arg("-o")
                .arg(&output)
                .args(extra)
                .output()
                .unwrap()
        };
        let stdout = format!("Exported 1 entries to {} (ja→es)\n", output.display());
        result(run(&[]), 0, &stdout, "");
        let bytes = std::fs::read(&output).unwrap();
        result(
            run(&["--source-lang", "not_a_language", "--overwrite"]),
            0,
            &stdout,
            "",
        );
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        result(f.command("import", format).arg("-i").arg(&output)
            .args(["--source-lang", "not_a_language", "--keep-existing", "--dry-run"]).output().unwrap(), 0,
            "Would replace 0 translation(s):\nWould fill 0 empty row(s)\nUnchanged 1 (already identical)\nDry run: nothing was saved.\n", "");
        result(
            f.command("import", format)
                .arg("-i")
                .arg(&output)
                .args(["--source-lang", "not_a_language"])
                .output()
                .unwrap(),
            0,
            &format!(
                "Imported 1 translations from {}\nUnchanged 1 (already identical)\n",
                output.display()
            ),
            "",
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn cli_help_names_restricted_tmx_and_source_flag_scope() {
    for action in ["import", "export"] {
        let output = Command::new(env!("CARGO_BIN_EXE_locust"))
            .args([action, "--help"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("restricted bilingual TMX"));
        assert!(help.contains("--source-lang <SOURCE_LANG>"));
        assert!(help.contains("required for TMX; ignored for PO/XLIFF"));
    }
}
