use std::path::{Path, PathBuf};

use comfy_table::Table;
use locust_core::backup::BackupManager;
use locust_core::database::{Database, EntryFilter};

use crate::util::{locust_backup_root, open_existing_project};
use crate::PatchIdentityArgs;

impl PatchIdentityArgs {
    fn resolve(&self, path: &Path) -> anyhow::Result<locust_core::patch::GameIdentity> {
        let (game, detected) = locust_core::patch::identity::resolve_patch_identity(
            self.rj.as_deref(),
            &self.store_id,
            self.game_version.as_deref(),
            !self.no_detect_id,
            path,
        )?;
        if let Some(code) = detected {
            println!("Detected DLsite id {code} from path");
        }
        Ok(game)
    }
}

pub(crate) fn cmd_patch(
    game_path: PathBuf,
    project: PathBuf,
    lang: Option<String>,
    output: Option<PathBuf>,
    astro: Option<PathBuf>,
    pristine: Option<PathBuf>,
    identity: PatchIdentityArgs,
) -> anyhow::Result<()> {
    use locust_core::patch::{pack_with_pristine_backup, PackOptions};

    let game = identity.resolve(&game_path)?;
    let db = open_existing_project(&project)?;
    let out = output.unwrap_or_else(|| {
        let base = game_path.file_name().unwrap_or_default().to_string_lossy();
        let suffix = lang.as_deref().map(|l| format!("-{l}")).unwrap_or_default();
        PathBuf::from(format!("{base}{suffix}-patch.zip"))
    });
    let engine = detect_engine_label(&db, &game_path)?;

    if let Some(path) = &astro {
        validate_astro_output(path, &game_path, &project, &out, pristine.as_deref())?;
        for language in db.list_recorded_langs()? {
            if let Some(root) = db
                .get_injection(language.as_deref())?
                .and_then(|record| record.pristine_backup)
                .and_then(|backup| backup.storage_root)
            {
                locust_core::patch::ensure_pack_output_outside(path, &root)?;
            }
        }
    }

    let backup_store = BackupManager::new(locust_backup_root());
    let report = pack_with_pristine_backup(
        &db,
        PackOptions {
            game: Some(game),
            game_path: game_path.clone(),
            lang: lang.clone(),
            output: out,
            pristine,
            engine: Some(engine.as_deref().unwrap_or("unknown").to_string()),
            project: project.clone(),
            require_pristine: false,
        },
        &backup_store,
        None,
        true,
    )?;

    for m in &report.messages {
        println!("note: {m}");
    }

    if let Some(astro_path) = astro {
        write_astro_stub(
            &astro_path,
            &game_path,
            report.recording_lang.as_deref(),
            Path::new(&report.output_path),
            engine.as_deref(),
        )?;
        println!("Astro stub written to {}", astro_path.display());
    }

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Patch file", &report.output_path]);
    table.add_row(vec![
        "Recording",
        &report
            .recording_lang
            .clone()
            .unwrap_or_else(|| "(unspecified)".into()),
    ]);
    table.add_row(vec!["Recorded root", &report.recorded_root]);
    table.add_row(vec!["Files packed", &report.files_packed.to_string()]);
    table.add_row(vec![
        "Stored translations",
        &report.translated_strings.to_string(),
    ]);
    table.add_row(vec![
        "Size",
        &format!("{:.1} KB", report.size_bytes as f64 / 1024.0),
    ]);
    table.add_row(vec!["Patch id", &report.patch_id]);
    table.add_row(vec!["Version", &report.patch_version]);
    table.add_row(vec!["Tier", &report.tier]);
    println!("{table}");

    Ok(())
}

fn validate_astro_output(
    path: &Path,
    game_path: &Path,
    project: &Path,
    zip: &Path,
    pristine: Option<&Path>,
) -> anyhow::Result<()> {
    locust_core::patch::release_entry::validate_release_entry_output(
        path,
        game_path,
        project,
        zip,
        pristine,
        &locust_backup_root(),
    )
}

fn write_astro_stub(
    path: &Path,
    game_path: &Path,
    lang: Option<&str>,
    zip_path: &Path,
    engine: Option<&str>,
) -> anyhow::Result<()> {
    locust_core::patch::release_entry::write_release_entry(
        path,
        game_path,
        lang,
        zip_path,
        engine.unwrap_or("other"),
    )
}

fn detect_engine_label(db: &Database, game_path: &Path) -> anyhow::Result<Option<String>> {
    // File-source formats need not detect their Replace output directory, and
    // a shared parent may detect an unrelated engine. Prefer persisted provenance.
    let entries = db.get_entries(&EntryFilter::default())?;
    let mut formats = entries
        .iter()
        .filter_map(|entry| {
            entry
                .metadata
                .get(locust_core::models::FORMAT_ID_METADATA_KEY)
                .and_then(|value| value.as_str())
        })
        .filter(|id| !id.trim().is_empty());
    if let Some(format) = formats.next() {
        if formats.all(|other| other == format) {
            return Ok(Some(format.to_string()));
        }
    }

    let registry = locust_formats::default_registry();
    Ok(registry.detect(game_path).map(|p| p.id().to_string()))
}

pub(crate) fn cmd_apply(
    game_path: PathBuf,
    zip: Option<PathBuf>,
    url: Option<String>,
    force: bool,
    confirm_legacy: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    use locust_core::patch::{apply, ApplyOptions};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }

    // Keep download tempdir alive for the whole apply.
    let mut _download_guard: Option<tempfile::TempDir> = None;
    let zip_path = match (zip, url) {
        (Some(p), None) => {
            if !p.is_file() {
                anyhow::bail!("patch zip not found: {}", p.display());
            }
            p
        }
        (None, Some(u)) => {
            let dir = tempfile::tempdir()?;
            let dest = dir.path().join("locust-patch.zip");
            println!("downloading {u} …");
            download_patch_zip(&u, &dest)?;
            println!("saved {}", dest.display());
            _download_guard = Some(dir);
            dest
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("pass either a local zip path or --url, not both");
        }
        (None, None) => {
            anyhow::bail!("patch zip path or --url is required");
        }
    };

    let opts = ApplyOptions {
        force,
        confirm_legacy,
        dry_run,
    };
    let report = apply(&game_path, &zip_path, opts, |p| {
        println!("[{}/{}] {} ({})", p.current, p.total, p.path, p.phase);
    })?;

    if !report.user_edits_overwritten.is_empty() {
        println!(
            "warning: overwriting {} user-edited added file(s):",
            report.user_edits_overwritten.len()
        );
        for p in &report.user_edits_overwritten {
            println!("  {p}");
        }
    }
    for m in &report.messages {
        println!("{m}");
    }
    println!(
        "patch {}@{} {} — replaced {}, added {} (baseline: {:?})",
        report.patch_id,
        report.patch_version,
        if report.dry_run { "planned" } else { "applied" },
        report.replaced,
        report.added,
        report.baseline
    );
    Ok(())
}

/// Download a patch zip with per-hop network validation and pinned DNS answers.
fn download_patch_zip(url: &str, dest: &Path) -> anyhow::Result<()> {
    download_patch_zip_with_policy(url, dest, |addr| {
        locust_core::patch::net_policy::is_public_address(addr.ip())
    })
}

fn download_patch_zip_with_policy<F: Fn(std::net::SocketAddr) -> bool>(
    url: &str,
    dest: &Path,
    allow: F,
) -> anyhow::Result<()> {
    use locust_core::patch::net_policy;
    use std::net::ToSocketAddrs;

    let max_bytes = locust_core::patch::zipsec::max_download_bytes();
    let mut parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;
    let mut redirects = 0;
    let mut resp = loop {
        net_policy::validate_scheme(parsed.scheme(), None).map_err(anyhow::Error::msg)?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow::anyhow!(net_policy::DNS_FAILED))?
            .trim_start_matches('[')
            .trim_end_matches(']');
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!(net_policy::DNS_FAILED))?;
        let addresses: Vec<_> = (host, port)
            .to_socket_addrs()
            .map_err(|_| anyhow::anyhow!(net_policy::DNS_FAILED))?
            .collect();
        net_policy::validate_addresses(&addresses, redirects > 0, &allow)
            .map_err(anyhow::Error::msg)?;

        // A fresh, direct client per hop prevents DNS rebinding and connection
        // pool reuse. Keep the original hostname for Host and TLS validation.
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30 * 60))
            .connect_timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .resolve_to_addrs(host, &addresses)
            .user_agent(format!("locust/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let response = client.get(parsed.clone()).send()?.error_for_status()?;
        if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            break response;
        }
        if redirects == net_policy::MAX_REDIRECTS {
            anyhow::bail!(net_policy::REDIRECT_LIMIT);
        }
        let next = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|location| parsed.join(location).ok())
            .ok_or_else(|| anyhow::anyhow!(net_policy::INVALID_REDIRECT))?;
        net_policy::validate_scheme(next.scheme(), Some(parsed.scheme()))
            .map_err(anyhow::Error::msg)?;
        parsed = next;
        redirects += 1;
    };
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            anyhow::bail!("remote zip too large: {len} bytes (max {max_bytes})");
        }
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(dest)?;
    let mut written: u64 = 0;
    let mut buf = [0u8; 1024 * 256];
    loop {
        let n = {
            use std::io::Read;
            resp.read(&mut buf)?
        };
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > max_bytes {
            let _ = std::fs::remove_file(dest);
            anyhow::bail!("download exceeded {max_bytes} bytes — aborted");
        }
        use std::io::Write;
        file.write_all(&buf[..n])?;
    }
    if written == 0 {
        let _ = std::fs::remove_file(dest);
        anyhow::bail!("download produced an empty file");
    }
    println!("downloaded {written} bytes");
    Ok(())
}

pub(crate) fn cmd_patch_rollback(
    game_path: PathBuf,
    force: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    use locust_core::patch::{preview_rollback, rollback, RollbackOptions};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }
    let opts = RollbackOptions {
        delete_modified_added: force,
    };
    let report = if dry_run {
        preview_rollback(&game_path, opts)?
    } else {
        rollback(&game_path, opts)?
    };
    if !report.aborted_edited.is_empty() {
        println!("rollback aborted — edited added files need --force:");
        for p in &report.aborted_edited {
            println!("  {p}");
        }
        anyhow::bail!("rollback refused: modified added files were preserved; use --force to authorize deletion");
    }
    for m in &report.messages {
        println!("{m}");
    }
    if !report.torn_deleted.is_empty() {
        println!(
            "changed added files {} with explicit force:",
            if dry_run {
                "would be deleted"
            } else {
                "deleted"
            }
        );
        for p in &report.torn_deleted {
            println!("  {p}");
        }
    }
    if dry_run {
        println!(
            "rollback planned — restore {}, delete {}",
            report.restored, report.deleted
        );
    } else {
        println!(
            "rollback complete — restored {}, deleted {}",
            report.restored, report.deleted
        );
    }
    Ok(())
}

pub(crate) fn cmd_patch_status(game_path: PathBuf, verify: bool) -> anyhow::Result<()> {
    use locust_core::patch::{GameLock, PatchStatus, PatchStore};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }
    // Keep an apply or rollback from changing files during the hash check.
    let _lock = if verify {
        Some(GameLock::acquire(&game_path)?)
    } else {
        None
    };
    let store = PatchStore::new(&game_path);
    let injections = store.injection_status()?;
    match store.status()? {
        PatchStatus::NotPatched if verify => anyhow::bail!("no completed patch to verify"),
        PatchStatus::NotPatched => {
            if injections.injections.is_empty() && !injections.injection_pending {
                println!("Original game (no recorded Locust changes)");
            }
        }
        PatchStatus::Patched(r) => {
            if verify {
                verify_patch_receipt_files(&game_path, &r)?;
            }
            println!(
                "patched: {}@{} (engine {}, lang {}, baseline {:?}, forced={})",
                r.patch_id, r.patch_version, r.engine, r.language, r.baseline, r.forced
            );
            println!(
                "  replaced {} file(s), added {} file(s), applied_at {}",
                r.replaced.len(),
                r.added.len(),
                r.applied_at
            );
        }
        PatchStatus::Interrupted(_) if verify => {
            anyhow::bail!("patch apply was interrupted; roll back before verifying")
        }
        PatchStatus::Interrupted(j) => {
            println!(
                "INTERRUPTED apply of {} — run `locust patch-rollback \"{}\"`",
                j.patch_id,
                game_path.display()
            );
        }
        PatchStatus::Unknown if verify => anyhow::bail!("no usable patch receipt to verify"),
        PatchStatus::Unknown => {
            println!(
                "unknown — .locust/ present but no usable receipt (run patch-status after apply, \
                 or patch-rollback if a backup exists)"
            );
        }
    }
    if injections.injection_pending {
        println!("INJECTION UNFINISHED — inspect with locust inject-status and recover with locust inject-recover");
    }
    for injection in injections.injections {
        let label = match injection.mode {
            Some(locust_core::injection_transaction::InjectionMode::Direct) => {
                "Translated (direct)"
            }
            Some(locust_core::injection_transaction::InjectionMode::Add) => "Language added",
            None => "Game modified",
        };
        println!(
            "{}: {} ({} file(s), time {}, transaction {})",
            label,
            injection
                .language
                .as_deref()
                .unwrap_or("language unavailable"),
            injection.changed_files,
            injection
                .applied_at
                .map(|time| time.to_rfc3339())
                .unwrap_or_else(|| "unavailable".into()),
            injection.transaction_id
        );
    }
    Ok(())
}

fn verify_patch_receipt_files(
    game_path: &Path,
    receipt: &locust_core::patch::Receipt,
) -> anyhow::Result<()> {
    use locust_core::patch::{verify_receipt_files, PatchStore, ReceiptVerificationMode};

    let report = verify_receipt_files(
        &PatchStore::new(game_path),
        receipt,
        ReceiptVerificationMode::Full,
    )?;
    let changed: std::collections::HashSet<_> = report.changed.iter().collect();
    let missing: std::collections::HashSet<_> = report.missing.iter().collect();
    let files = receipt
        .replaced
        .iter()
        .map(|file| &file.path)
        .chain(receipt.added.iter().map(|file| &file.path));
    for path in files {
        if missing.contains(path) {
            println!("missing: {path}");
        } else if changed.contains(path) {
            println!("changed: {path}");
        }
    }
    let differing = report.changed.len() + report.missing.len();
    if differing > 0 {
        anyhow::bail!("{differing} patch file(s) differ from the receipt");
    }
    println!(
        "verified {} patch file(s) against the receipt",
        report.checked
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    // ─── cmd_patch packaging tests: `patch` packs EXCLUSIVELY from the
    // recording injection persisted (root + rel + hash per language key);
    // every mismatch is a loud error naming a remedy that works ─────────────

    use super::*;
    use crate::test_support::{patch_test_tempdir, save_translated, TestProfile};
    use crate::{Cli, Commands};
    use clap::Parser;
    use locust_core::database::sha256_hex;
    use locust_core::models::{StringEntry, StringStatus};
    use std::fs;

    #[test]
    fn patch_download_refuses_private_redirect_before_connecting() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let source = listener.local_addr().unwrap();
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_addr = target.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).unwrap() > 0);
            write!(socket, "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/private.zip\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("patch.zip");
        let error = download_patch_zip_with_policy(
            &format!("http://localhost:{}/patch.zip", source.port()),
            &dest,
            |addr| addr.port() == source.port(),
        )
        .unwrap_err()
        .to_string();
        server.join().unwrap();
        assert_eq!(
            error,
            "patch URL redirects to a private network address; download refused"
        );
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(!dest.exists());
    }

    #[test]
    fn patch_download_refuses_private_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("patch.zip");
        for url in ["http://127.0.0.1:1/p.zip", "http://[::1]:1/p.zip"] {
            let error = download_patch_zip(url, &dest).unwrap_err().to_string();
            assert!(error.contains("private network address"), "{error}");
            assert!(!dest.exists());
        }
    }

    #[test]
    fn download_patch_zip_rejects_non_http_schemes() {
        // A patch URL is attacker-supplied in the CDN case; only http(s) may
        // ever be fetched, and nothing is written when the scheme is refused.
        let dir = std::env::temp_dir().join(format!("locust_dl_{}", uuid::Uuid::new_v4()));
        let dest = dir.join("p.zip");
        for url in [
            "file:///C:/Windows/win.ini",
            "ftp://example.com/a.zip",
            "data:application/zip;base64,UEsDBA==",
        ] {
            let e = download_patch_zip(url, &dest)
                .expect_err("must refuse non-http scheme")
                .to_string();
            assert!(e.contains("http"), "{url} -> {e}");
            assert!(!dest.exists(), "{url} wrote a file");
        }
    }

    fn astro_test_zip(directory: &Path) -> PathBuf {
        use std::io::Write;
        let path = directory.join("patch.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        zip.start_file(
            "locust-patch.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1, "patch_id": "test", "game_name": "game",
            "engine": "renpy", "language": "es", "patch_version": "1.0.0",
            "generator_version": "0.1.0", "created_at": "t", "files": []
        });
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn patch_engine_label_prefers_unambiguous_project_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (game, _, _, _) = make_renpy_game(dir.path());
        let file_source_output = dir.path().join("replace-es");
        fs::create_dir(&file_source_output).unwrap();
        let db = Database::open_in_memory().unwrap();
        // Repeated IDs and unmarked legacy rows do not introduce another format.
        db.save_entries(&[
            StringEntry::new("a", "Hello", "game.pck/main.dtl".into())
                .with_format_id("godot-dialogic"),
            StringEntry::new("b", "World", "game.pck/main.dtl".into())
                .with_format_id("godot-dialogic"),
            StringEntry::new("c", "Legacy", "old.txt".into()),
        ])
        .unwrap();
        // Neither an undetectable output nor a detectable sibling engine
        // should override the project's format.
        for (index, game) in [file_source_output, game].into_iter().enumerate() {
            let engine = detect_engine_label(&db, &game).unwrap();
            assert_eq!(engine.as_deref(), Some("godot-dialogic"));
            let stub = dir.path().join(format!("release-{index}.md"));
            write_astro_stub(
                &stub,
                &game,
                Some("es"),
                &astro_test_zip(dir.path()),
                engine.as_deref(),
            )
            .unwrap();
            assert!(fs::read_to_string(stub)
                .unwrap()
                .contains("\nengine: \"godot-dialogic\" "));
        }
    }

    #[test]
    fn patch_engine_label_falls_back_for_ambiguous_or_missing_formats() {
        let dir = tempfile::tempdir().unwrap();
        let (game, _, _, _) = make_renpy_game(dir.path());
        let unknown = dir.path().join("unknown");
        fs::create_dir(&unknown).unwrap();
        for formats in [
            vec![],
            vec![None],
            vec![Some("godot-dialogic"), Some("html-game")],
        ] {
            let db = Database::open_in_memory().unwrap();
            for (index, format) in formats.into_iter().enumerate() {
                let mut row = StringEntry::new(index.to_string(), "Hello", "file".into());
                if let Some(format) = format {
                    row = row.with_format_id(format);
                }
                db.save_entries(&[row]).unwrap();
            }
            assert_eq!(
                detect_engine_label(&db, &game).unwrap().as_deref(),
                Some("renpy")
            );
            assert_eq!(detect_engine_label(&db, &unknown).unwrap(), None);
        }
    }

    #[test]
    fn patch_engine_label_ignores_invalid_format_metadata() {
        use locust_core::models::FORMAT_ID_METADATA_KEY;

        let dir = tempfile::tempdir().unwrap();
        let (game, _, _, _) = make_renpy_game(dir.path());
        let db = Database::open_in_memory().unwrap();
        for (index, value) in [
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!(""),
            serde_json::json!("   "),
        ]
        .into_iter()
        .enumerate()
        {
            let mut row = StringEntry::new(index.to_string(), "Hello", "file".into());
            row.metadata.insert(FORMAT_ID_METADATA_KEY.into(), value);
            db.save_entries(&[row]).unwrap();
        }
        assert_eq!(
            detect_engine_label(&db, &game).unwrap().as_deref(),
            Some("renpy")
        );
    }

    #[test]
    fn patch_engine_label_preserves_existing_engine_id() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&[
            StringEntry::new("a", "Hello", "game/script.rpy".into()).with_format_id("renpy")
        ])
        .unwrap();
        let engine = detect_engine_label(&db, dir.path()).unwrap();
        assert_eq!(engine.as_deref(), Some("renpy"));
        let stub = dir.path().join("release.md");
        write_astro_stub(
            &stub,
            dir.path(),
            Some("es"),
            &astro_test_zip(dir.path()),
            engine.as_deref(),
        )
        .unwrap();
        assert!(fs::read_to_string(stub)
            .unwrap()
            .contains("\nengine: \"renpy\" "));
    }

    #[test]
    fn astro_stub_guides_players_to_rule95_patcher() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_astro_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let stub = dir.join("release.md");
        let game = dir.join("MyGame");
        fs::create_dir_all(&game).unwrap();
        write_astro_stub(&stub, &game, Some("es"), &astro_test_zip(&dir), None).unwrap();
        let md = fs::read_to_string(&stub).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(md.contains("gameVersion: \"TODO\"\n"));
        assert!(md.contains("  fingerprint: []\n"));
        assert!(!md.contains("rjCode:"));
        assert!(
            md.contains("Rule95 Patcher"),
            "must name the player app:\n{md}"
        );
        assert!(md.contains("patch ZIP"), "must name the patch file:\n{md}");
        assert!(
            md.contains("game folder"),
            "must name the game folder:\n{md}"
        );
        assert!(md.contains("Undo last patch"), "must explain undo:\n{md}");
        assert!(
            !md.contains("locust apply"),
            "must not require the CLI:\n{md}"
        );
        assert!(
            !md.contains("extract over"),
            "must not suggest manual unzip:\n{md}"
        );
        assert!(
            md.to_lowercase().contains("backup"),
            "must mention restorable backup:\n{md}"
        );
    }

    /// A Ren'Py-shaped game tree with one loose script (a state extraction
    /// genuinely ingests: loose `.rpy` files become entries with their own
    /// path), plus a project database holding its translated entry.
    fn make_renpy_game(base: &std::path::Path) -> (PathBuf, PathBuf, PathBuf, String) {
        let game_dir = base.join("mygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        let contents = "label start:\n    \"Hola\"\n".to_string();
        fs::write(&script, &contents).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "script.rpy#2", "Hello", &script, "Hola");
        drop(db);
        (game_dir, script, db_path, contents)
    }

    #[test]
    fn patch_identity_flags_parse_and_normalize() {
        let cli = Cli::try_parse_from([
            "locust",
            "patch",
            "game",
            "-P",
            "project.db",
            "--rj",
            "re01234567",
            "--store-id",
            "Steam=123",
            "--store-id",
            "VNDB=v42",
            "--game-version",
            " 1.2 ",
            "--no-detect-id",
        ])
        .unwrap();
        let Commands::Patch { identity, .. } = cli.command else {
            panic!("expected patch");
        };
        let game = identity.resolve(Path::new("RJ123456")).unwrap();
        assert_eq!(game.store_ids["dlsite"], "RE01234567");
        assert_eq!(game.store_ids["steam"], "123");
        assert_eq!(game.store_ids["vndb"], "v42");
        assert_eq!(game.game_version.as_deref(), Some("1.2"));
        assert!(identity.no_detect_id);
    }

    #[test]
    fn patch_identity_rejects_invalid_store_ids_duplicates_and_empty_version() {
        for (ids, rj, version, expected) in [
            (vec!["steam"], None, None, "store=id"),
            (vec!["=123"], None, None, "empty"),
            (vec!["steam= "], None, None, "empty"),
            (vec![" =123"], None, None, "empty"),
            (vec!["dlsite=XX123456"], None, None, "DLsite"),
            (vec!["steam=1", "STEAM=2"], None, None, "duplicate"),
            (vec!["dlsite=RJ123456"], Some("RJ123456"), None, "duplicate"),
            (vec![], None, Some(" \t "), "game version"),
        ] {
            let args = PatchIdentityArgs {
                rj: rj.map(str::to_string),
                store_id: ids.into_iter().map(str::to_string).collect(),
                game_version: version.map(str::to_string),
                no_detect_id: false,
            };
            let error = args.resolve(Path::new("RJ123456")).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn patch_identity_detection_can_be_disabled_or_overridden() {
        let path = Path::new("D:/RJ123456/[RE01234567] Game/");
        let mut args = PatchIdentityArgs::default();
        assert_eq!(
            args.resolve(path).unwrap().store_ids["dlsite"],
            "RE01234567"
        );
        args.no_detect_id = true;
        assert!(args.resolve(path).unwrap().store_ids.is_empty());
        args.rj = Some("vj123456".into());
        assert_eq!(args.resolve(path).unwrap().store_ids["dlsite"], "VJ123456");
        args.rj = None;
        args.no_detect_id = false;
        args.store_id = vec!["DLSITE=bj123456".into()];
        assert_eq!(args.resolve(path).unwrap().store_ids["dlsite"], "BJ123456");
    }

    #[test]
    fn patch_invalid_rj_fails_before_packing_or_opening_project() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("patch.zip");
        let error = cmd_patch(
            dir.path().join("missing-game"),
            dir.path().join("missing.db"),
            None,
            Some(output.clone()),
            None,
            None,
            PatchIdentityArgs {
                rj: Some("RJ12345".into()),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("DLsite") && error.contains("6 or 8 digits"),
            "{error}"
        );
        assert!(!output.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn patch_identity_and_astro_metadata_match_written_zip() {
        let _profile = TestProfile::new();
        let base = tempfile::tempdir().unwrap();
        let (game, script, db_path, _) = make_renpy_game(base.path());
        let pristine = base.path().join("pristine");
        fs::create_dir_all(pristine.join("game")).unwrap();
        let original = b"label start:\n    \"Hello, adventurer!\"\n";
        fs::write(pristine.join("game/script.rpy"), original).unwrap();
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game, &[script]).unwrap();
        drop(db);
        let output = base.path().join("patch.zip");
        let stub = base.path().join("release.md");
        cmd_patch(
            game,
            db_path,
            None,
            Some(output.clone()),
            Some(stub.clone()),
            Some(pristine),
            PatchIdentityArgs {
                rj: Some("rj01234567".into()),
                store_id: vec!["Steam=123".into()],
                game_version: Some(" 1.2 \"test\" ".into()),
                no_detect_id: false,
            },
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&output).unwrap()).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_reader(zip.by_name("locust-patch.json").unwrap()).unwrap();
        let fingerprint = serde_json::json!([{
            "path": "game/script.rpy", "size": original.len(), "sha256": sha256_hex(original)
        }]);
        assert_eq!(
            manifest["game"],
            serde_json::json!({
                "store_ids": {"dlsite": "RJ01234567", "steam": "123"},
                "game_version": "1.2 \"test\"", "fingerprint": fingerprint
            })
        );
        let md = fs::read_to_string(stub).unwrap();
        let bytes = fs::read(output).unwrap();
        assert!(md.contains("rjCode: \"RJ01234567\"\n"), "{md}");
        assert!(md.contains("gameVersion: \"1.2 \\\"test\\\"\"\n"), "{md}");
        let expected = format!("patch:\n  id: {}\n  file: \"patch.zip\"\n  size: {}\n  sha256: \"{}\"\n  language: \"es\"\n  fingerprint:\n    - path: \"game/script.rpy\"\n      size: {}\n      sha256: \"{}\"\n",
            manifest["patch_id"], bytes.len(), sha256_hex(&bytes), original.len(), sha256_hex(original));
        assert!(md.contains(&expected), "{md}");
        assert!(md.contains("Rule95 Patcher"));
    }

    #[test]
    fn test_patch_astro_refuses_protected_or_existing_outputs_before_packing() {
        let _profile = TestProfile::new();
        for case in [
            "game",
            "new_game_file",
            "zip",
            "database",
            "wal",
            "note",
            "pristine",
        ] {
            let base_guard = patch_test_tempdir();
            let base = base_guard.path().to_path_buf();
            let (game, script, db_path, contents) = make_renpy_game(&base);
            let db = Database::open(&db_path).unwrap();
            db.record_injection(Some("es"), &game, std::slice::from_ref(&script))
                .unwrap();
            drop(db);
            let zip = base.join("patch.zip");
            let pristine = base.join("pristine");
            fs::create_dir(&pristine).unwrap();
            let note = base.join("release.md");
            fs::write(&note, "user notes").unwrap();
            let output = match case {
                "game" => script.clone(),
                "new_game_file" => game.join("new-release.md"),
                "zip" => base.join(".").join("patch.zip"),
                "database" => db_path.clone(),
                "wal" => PathBuf::from(format!("{}-wal", db_path.display())),
                "pristine" => pristine.join("release.md"),
                _ => note.clone(),
            };
            let previous = fs::read(&output).ok();
            let database = fs::read(&db_path).unwrap();
            let result = cmd_patch(
                game,
                db_path.clone(),
                Some("es".into()),
                Some(zip.clone()),
                Some(output.clone()),
                Some(pristine),
                PatchIdentityArgs::default(),
            );
            assert!(result.is_err(), "{case} unexpectedly accepted");
            assert!(
                !zip.exists(),
                "{case} packed a ZIP before refusing the stub"
            );
            assert_eq!(
                fs::read(&output).ok(),
                previous,
                "{case} changed destination"
            );
            assert_eq!(fs::read(&db_path).unwrap(), database);
            assert_eq!(fs::read_to_string(script).unwrap(), contents);
            let _ = fs::remove_dir_all(base);
        }
    }

    #[test]
    fn astro_stub_uses_the_auto_selected_recording_language() {
        let _profile = TestProfile::new();
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("fr"), &game, &[script]).unwrap();
        drop(db);
        let stub = base.join("release.md");
        cmd_patch(
            game,
            db_path,
            None,
            Some(base.join("patch.zip")),
            Some(stub.clone()),
            None,
            PatchIdentityArgs::default(),
        )
        .unwrap();
        assert!(fs::read_to_string(stub)
            .unwrap()
            .contains("Translation into fr"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn astro_stub_refuses_existing_file_even_without_preflight() {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("release.md");
        fs::write(&stub, "user notes").unwrap();
        let zip = astro_test_zip(dir.path());
        assert!(write_astro_stub(&stub, dir.path(), Some("es"), &zip, None).is_err());
        assert_eq!(fs::read_to_string(&stub).unwrap(), "user notes");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn test_patch_packs_the_recorded_files_and_bytes() {
        let _profile = TestProfile::new();
        // A recording exists for one key ("es", from a direct inject on the
        // game root); `patch` without -l resolves the single key and packs
        // exactly the recorded rels, byte-for-byte.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, contents) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(
            game_dir,
            db_path,
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .unwrap();

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut zf = archive
            .by_name("game/script.rpy")
            .expect("game/script.rpy must be packed in the archive");
        let mut read_back = String::new();
        use std::io::Read as _;
        zf.read_to_string(&mut read_back).unwrap();
        assert_eq!(read_back, contents);
    }

    #[cfg(windows)]
    #[test]
    fn test_patch_resolves_when_game_path_case_differs_from_record_time() {
        let _profile = TestProfile::new();
        // The recorded root and the patch-time game path are the same on-disk
        // directory spelled with different case. The identity check must fold
        // case where the filesystem does, not refuse a spelling difference.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let respelled = base.join("MYGAME");
        let out_zip = base.join("out-patch.zip");
        cmd_patch(
            respelled,
            db_path,
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect("a case-divergent spelling of the recorded root must still pack");

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.by_name("game/script.rpy").is_ok());
    }

    #[test]
    fn test_patch_all_rpa_database_packs_recorded_injection_output() {
        let _profile = TestProfile::new();
        // Archive-shipped Ren'Py game: EVERY database entry names the .rpa it
        // was READ from, while injection wrote loose .rpy files that
        // extraction deliberately never re-ingests (it skips `zzz_locust*`).
        // The patch must come from what injection RECORDED, not from entries.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let rpa_path = game_sub.join("scripts.rpa");
        fs::write(&rpa_path, b"RPA-3.0 fake original archive bytes").unwrap();
        // What injection actually wrote for the archive content.
        let loose = game_sub.join("script.rpy");
        fs::write(&loose, "label start:\n    \"Hola\"\n").unwrap();
        let filter = game_sub.join("zzz_locust_translate.rpy");
        fs::write(&filter, "# runtime filter\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "scripts.rpa#script.rpy#2", "Hello", &rpa_path, "Hola");
        db.record_injection(Some("es"), &game_dir, &[loose.clone(), filter.clone()])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(
            game_dir,
            db_path,
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect("an all-archive database with recorded injection output must pack");

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "game/script.rpy"),
            "the loose .rpy injection wrote must be packed, got: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "game/zzz_locust_translate.rpy"),
            "the generated filter file must be packed, got: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.ends_with(".rpa")),
            "the source archive was never recorded as written, so it must not pack: {names:?}"
        );
    }

    #[test]
    fn test_patch_without_recording_names_the_exact_inject_command() {
        let _profile = TestProfile::new();
        // A legacy database (or one whose old-format recording was dropped by
        // the migration): entries are translated, but nothing was ever
        // recorded. There is deliberately NO entry-derived fallback — the
        // fallback was the mechanism of every silently-wrong patch — so this
        // must hard-error naming the exact command that records an injection.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, _script, db_path, _) = make_renpy_game(&base);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("no recording must be a hard error, never an entry-derived guess");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection has been recorded"),
            "error must say what is missing: {msg}"
        );
        let advised = format!(
            "locust inject \"{}\" -P \"{}\" --direct -l es",
            game_dir.display(),
            db_path.display()
        );
        assert!(
            msg.contains(&advised),
            "error must name the exact inject command\nwant: {advised}\ngot: {msg}"
        );
        assert_eq!(
            msg.matches("This engine writes translations into the ORIGINAL game files")
                .count(),
            1,
            "restore advice must appear once: {msg}"
        );
        assert!(!out_zip.exists(), "no archive may be written on this path");
    }

    #[test]
    fn test_patch_key_miss_is_a_loud_error_listing_recorded_keys() {
        let _profile = TestProfile::new();
        // A recording exists — for a DIFFERENT language. The old code fell
        // back to the entry-derived list silently; now the mismatch is loud,
        // lists what IS recorded, and offers only remedies that work from
        // this state ( `-l fr` is offered because fr's root IS this tree).
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("fr"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a language with no recording must be a hard error");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection recorded for language \"es\""),
            "error must name the missing key: {msg}"
        );
        assert!(
            msg.contains("\"fr\""),
            "error must list the recorded keys: {msg}"
        );
        assert!(
            msg.contains("--direct -l es"),
            "error must advise recording the requested language: {msg}"
        );
        assert!(
            msg.contains("-l fr"),
            "error must offer the recorded key whose root is this tree: {msg}"
        );
    }

    #[test]
    fn test_patch_null_key_is_packed_without_lang_and_never_by_a_named_lang() {
        let _profile = TestProfile::new();
        // `--direct` without -l records under the reserved language-unspecified
        // key: `patch` without -l packs it, `patch -l es` must NOT silently
        // match it.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(None, &game_dir, &[script]).unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect("the language-unspecified recording must pack without -l");
        assert!(out_zip.exists());

        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(base.join("other.zip")),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a named language must never silently match the unspecified key");
        let msg = err.to_string();
        assert!(
            msg.contains("(unspecified)"),
            "error must list the unspecified key so the state is visible: {msg}"
        );
        assert!(
            msg.contains("without -l"),
            "the working remedy here is re-running patch without -l: {msg}"
        );
    }

    #[test]
    fn test_patch_without_lang_and_multiple_keys_requires_an_explicit_choice() {
        let _profile = TestProfile::new();
        // Key set {es, (unspecified)} with `patch` invoked without -l is
        // claimed by two rules: "NULL key is matched by patch without -l" and
        // "multiple keys without -l is an error". The error wins — packing a
        // silent guess (or the union) is how mixed-language zips shipped.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        db.record_injection(None, &game_dir, &[script]).unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("multiple keys without -l must be a hard error, not a guess");
        let msg = err.to_string();
        assert!(msg.contains("es"), "error must list the named key: {msg}");
        assert!(
            msg.contains("(unspecified)"),
            "error must list the unspecified key too: {msg}"
        );
        assert!(
            msg.contains("-l"),
            "error must require an explicit -l: {msg}"
        );
        // "Pass -l <lang>" alone cannot reach the (unspecified) recording —
        // no -l value names the NULL key. The error must say how to reach it:
        // re-record it under a named key.
        assert!(
            msg.contains("re-record") && msg.contains("--direct -l <lang>"),
            "the unspecified recording must be reachable, not a dead entry \
             in a list: {msg}"
        );
        assert!(!out_zip.exists());

        // The advised choice unblocks.
        cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect("choosing a key with -l must unblock");
        assert!(out_zip.exists());
    }

    /// A Wolf-RPG-shaped game (engine class E5, an entry-tree writer whose
    /// originals are byte-patched in place) with one translated entry, for
    /// exercising the mutated-originals advice on `patch`'s error paths.
    fn make_wolf_game(base: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
        let game_dir = base.join("wolfgame");
        let data = game_dir.join("Data");
        fs::create_dir_all(&data).unwrap();
        let wolf_file = data.join("BasicData.wolf");
        fs::write(&wolf_file, locust_formats::wolf_rpg::build_test_fixture()).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "BasicData.wolf#0", "テストデータ", &wolf_file, "test");
        drop(db);
        (game_dir, wolf_file, db_path)
    }

    #[test]
    fn test_patch_no_recording_advice_warns_when_the_originals_may_be_mutated() {
        let _profile = TestProfile::new();
        // A legacy database on an already-injected entry-tree game: `patch`
        // advises a bare `--direct` re-run, but for engines that mutate the
        // originals that re-run writes 0 files and records nothing — the
        // identical error forever. The advice must carry the restore-first
        // note. `patch` cannot know whether a prior inject ran, so every
        // engine that writes in place carries the restore-first note.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, _wolf_file, db_path) = make_wolf_game(&base);

        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            None,
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("no recording must be a hard error");
        let msg = err.to_string();
        assert!(msg.contains("no injection has been recorded"), "{msg}");
        assert!(
            msg.contains("0 files written") && msg.contains("restore the original"),
            "the advice must explain the already-injected dead end and its way \
             out for an engine that mutates originals: {msg}"
        );

        // Every registered format writes in place, so the restore note is
        // no longer gated on the stale four-engine list.
        let base2_guard = patch_test_tempdir();
        let base2 = base2_guard.path().to_path_buf();
        let (game2, _script, db2, _) = make_renpy_game(&base2);
        let err2 = cmd_patch(
            game2,
            db2,
            Some("es".to_string()),
            None,
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .unwrap_err();
        assert!(err2.to_string().contains("restore the original"), "{err2}");

        let base3_guard = patch_test_tempdir();
        let base3 = base3_guard.path().to_path_buf();
        let game3 = base3.join("htmlgame");
        fs::create_dir_all(&game3).unwrap();
        fs::write(
            game3.join("index.html"),
            "<html><body><p>Hi there world</p></body></html>",
        )
        .unwrap();
        let db3 = base3.join("project.locust.db");
        let dbh = Database::open(&db3).unwrap();
        save_translated(
            &dbh,
            "index.html#0",
            "Hi there world",
            &game3.join("index.html"),
            "Hola",
        );
        drop(dbh);
        let err3 = cmd_patch(
            game3,
            db3,
            Some("es".to_string()),
            None,
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .unwrap_err();
        let msg3 = err3.to_string();
        assert!(msg3.contains("no injection has been recorded"), "{msg3}");
        assert!(
            msg3.contains("restore the original"),
            "html-game writes in place too, so the restore note must fire: {msg3}"
        );
    }

    #[test]
    fn test_patch_key_miss_advice_warns_when_the_originals_may_be_mutated() {
        let _profile = TestProfile::new();
        // A recording exists for another language on an entry-tree game: its
        // originals were provably byte-patched by that inject, so the advised
        // `--direct -l <missing>` re-run needs the same restore-first note.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, wolf_file, db_path) = make_wolf_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("fr"), &game_dir, &[wolf_file])
            .unwrap();
        drop(db);

        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            None,
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a key miss must be a hard error");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection recorded for language \"es\""),
            "{msg}"
        );
        assert!(
            msg.contains("0 files written") && msg.contains("restore the original"),
            "the advised --direct re-run dead-ends on the already-patched \
             originals without the restore note: {msg}"
        );
    }

    #[test]
    fn test_patch_refuses_recorded_rels_that_are_not_plain_relative_paths() {
        let _profile = TestProfile::new();
        // Record time cannot emit an absolute rel, but `patch` trusts DB TEXT
        // it did not derive (a shared .locust.db is a plausible input). An
        // absolute rel survives a ParentDir-only guard, and
        // `recording.root.join(abs)` REPLACES the root — the patch would read
        // a file outside the game tree and ship it, hash-verified from the
        // same row.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let secret = base.join("secret.txt");
        let payload: &[u8] = b"outside the game tree";
        fs::write(&secret, payload).unwrap();
        let abs_rel = secret.display().to_string().replace('\\', "/");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE injected_files SET rel = ?1, hash = ?2, size = ?3",
            rusqlite::params![abs_rel, sha256_hex(payload), payload.len() as i64],
        )
        .unwrap();
        drop(conn);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("an absolute recorded rel must be refused, never packed");
        assert!(
            err.to_string().contains("escapes the game root"),
            "the refusal must name the escape: {err}"
        );
        assert!(!out_zip.exists(), "no archive may ship an out-of-tree file");

        // Triangulate: a `..` rel is refused by the same guard.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE injected_files SET rel = '../evil.txt'", [])
            .unwrap();
        drop(conn);
        let err = cmd_patch(
            game_dir,
            db_path,
            None,
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a parent-dir rel must be refused");
        assert!(err.to_string().contains("escapes the game root"), "{err}");
        assert!(!out_zip.exists());
    }

    #[test]
    fn test_patch_counts_reviewed_and_approved_strings_as_translated() {
        let _profile = TestProfile::new();
        // The desktop Review page sets `approved` (Review.tsx); `patch`'s
        // pre-check must not send a fully reviewed/approved project back to
        // `locust translate` — that advice would re-translate finished work.
        for status in [StringStatus::Approved, StringStatus::Reviewed] {
            let base_guard = patch_test_tempdir();
            let base = base_guard.path().to_path_buf();
            let game_dir = base.join("mygame");
            let game_sub = game_dir.join("game");
            fs::create_dir_all(&game_sub).unwrap();
            let script = game_sub.join("script.rpy");
            fs::write(&script, "label start:\n    \"Hola\"\n").unwrap();
            let db_path = base.join("project.locust.db");
            let db = Database::open(&db_path).unwrap();
            let mut entry = StringEntry::new("script.rpy#2", "Hello", script.clone());
            entry.translation = Some("Hola".to_string());
            entry.status = status.clone();
            db.save_entries(&[entry]).unwrap();
            drop(db);

            let err = cmd_patch(
                game_dir,
                db_path,
                Some("es".to_string()),
                None,
                None,
                None,
                PatchIdentityArgs::default(),
            )
            .expect_err("no recording exists, so patch still errors — but LATER");
            let msg = err.to_string();
            assert!(
                msg.contains("no injection has been recorded"),
                "a {status:?} project must pass the translated-strings gate: {msg}"
            );
            assert!(
                !msg.contains("locust translate"),
                "re-translating finished work must never be the advice: {msg}"
            );
        }

        // Triangulate: a project with nothing translated still gets the gate.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let game_dir = base.join("mygame");
        fs::create_dir_all(game_dir.join("game")).unwrap();
        let script = game_dir.join("game").join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello\"\n").unwrap();
        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        db.save_entries(&[StringEntry::new("script.rpy#2", "Hello", script)])
            .unwrap();
        drop(db);
        let err = cmd_patch(
            game_dir,
            db_path,
            None,
            None,
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("nothing to pack yet"),
            "an untranslated project must still be told to translate: {err}"
        );
    }

    #[test]
    fn test_patch_refuses_the_wrong_tree_and_names_the_recorded_root() {
        let _profile = TestProfile::new();
        // R6: Replace-mode injection recorded under the per-language COPY;
        // the user points patch at the ORIGINAL. The old content-root anchor
        // silently re-read the rels out of the original — an untranslated zip
        // claiming "translated text only". Now: hard error naming the root.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, _script, db_path, _) = make_renpy_game(&base);
        // The Replace copy, injected and recorded.
        let copy_dir = base.join("out").join("mygame-es");
        let copy_sub = copy_dir.join("game");
        fs::create_dir_all(&copy_sub).unwrap();
        let copy_script = copy_sub.join("script.rpy");
        fs::write(&copy_script, "label start:\n    \"Hola\"\n").unwrap();
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &copy_dir, &[copy_script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("packing recorded rels out of a different tree must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains(&copy_dir.display().to_string()),
            "error must name the recorded root to point patch at: {msg}"
        );
        assert!(!out_zip.exists());

        // The advised command — patch pointed at the recorded root — unblocks.
        cmd_patch(
            copy_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect("patch pointed at the recorded root must pack");
        assert!(out_zip.exists());
    }

    #[test]
    fn test_patch_refuses_a_recorded_file_that_changed_since_injection() {
        let _profile = TestProfile::new();
        // The recording carries the hash of what injection wrote; a file that
        // no longer matches must fail loudly instead of shipping bytes nobody
        // verified (F8: nothing ever checked a packed file was the file
        // injection reported writing).
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);

        fs::write(&script, "label start:\n    \"Overwritten\"\n").unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a changed file must refuse to pack");
        let msg = err.to_string();
        assert!(
            msg.contains("changed on disk since injection"),
            "error must say what happened: {msg}"
        );
        assert!(
            msg.contains("game/script.rpy"),
            "error must name the changed rel: {msg}"
        );
        assert!(
            msg.contains("Restore the last recorded injected files"),
            "error must explain how to recover the recorded bytes: {msg}"
        );
        assert!(!out_zip.exists(), "no archive may be written on this path");
    }

    #[test]
    fn test_patch_errors_when_a_recorded_file_is_missing() {
        let _profile = TestProfile::new();
        // A recording names every file the patch promised to carry. A missing
        // one must be a hard error naming the path — the old flow listed it
        // under "Files not found on disk" and shipped the zip anyway.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);

        fs::remove_file(&script).unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a missing recorded file must be an error, not a silent skip");
        let msg = err.to_string();
        assert!(msg.contains("missing from disk"), "{msg}");
        assert!(
            msg.contains(&script.display().to_string()),
            "error must name the missing path: {msg}"
        );
        assert!(
            !out_zip.exists(),
            "no archive may be left behind when the recording cannot be honored"
        );
    }

    #[test]
    fn test_patch_failed_run_leaves_existing_output_untouched() {
        let _profile = TestProfile::new();
        // A previously published patch sits at `-o`. A re-run that fails the
        // recording verification (file missing since) must not truncate or
        // delete it: the archive is built in a temp file and renamed over the
        // destination only on success; the drop guard removes the temp.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);
        fs::remove_file(&script).unwrap();

        let out_zip = base.join("published-patch.zip");
        let published = b"previously published good patch bytes";
        fs::write(&out_zip, published).unwrap();

        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
            PatchIdentityArgs::default(),
        )
        .expect_err("a recording that cannot be honored must remain an error");
        assert!(err.to_string().contains("missing from disk"), "{err}");
        assert_eq!(
            fs::read(&out_zip).unwrap(),
            published,
            "the previously published patch must survive a failed run byte-for-byte"
        );
        // No temp litter either.
        let leftovers: Vec<_> = fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
