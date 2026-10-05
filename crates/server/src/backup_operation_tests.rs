use super::*;

#[tokio::test]
async fn c127_http_restore_returns_removed_and_kept_paths() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("renpy");
    std::fs::create_dir_all(game.join("game")).unwrap();
    let archive = include_bytes!("../../cli/tests/fixtures/c127-scripts.rpa");
    std::fs::write(game.join("game/scripts.rpa"), archive).unwrap();
    let mut state = create_test_state();
    Arc::get_mut(&mut state).unwrap().backup_manager =
        Arc::new(BackupManager::new(temp.path().join("backups")));
    let mut entries = state
        .format_registry
        .get("renpy")
        .unwrap()
        .extract(&game)
        .unwrap();
    assert_eq!(entries.len(), 1);
    entries[0].translation = Some("Translated archive dialogue.".into());
    entries[0].status = locust_core::models::StringStatus::Translated;
    state.db.save_entries(&entries).unwrap();
    locust_core::extraction::inject_direct(
        &state.format_registry,
        &state.db,
        &state.backup_manager,
        &game,
        "renpy",
        &["en".into()],
    )
    .unwrap();
    std::fs::write(game.join("save.dat"), "user save").unwrap();
    let backup = state.backup_manager.list_backups().unwrap().remove(0);
    let response = restore_backup(State(state), AxumPath(backup.id))
        .await
        .unwrap();
    let body = serde_json::to_value(&response.0).unwrap();
    assert_eq!(body["removed"].as_array().unwrap().len(), 1);
    assert_eq!(
        PathBuf::from(body["removed"][0].as_str().unwrap()),
        PathBuf::from("game/scripts/story.rpy")
    );
    assert_eq!(body["kept"][0]["path"], "save.dat");
    assert!(body["kept"][0]["reason"]
        .as_str()
        .unwrap()
        .contains("no recorded"));
    assert!(!game.join("game/scripts/story.rpy").exists());
    assert_eq!(
        std::fs::read(game.join("game/scripts.rpa")).unwrap(),
        archive
    );
    assert_eq!(
        std::fs::read_to_string(game.join("save.dat")).unwrap(),
        "user save"
    );
}

fn fixture() -> (tempfile::TempDir, Arc<AppState>, BackupEntry, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    std::fs::create_dir(&game).unwrap();
    let file = game.join("story.html");
    std::fs::write(&file, "Original").unwrap();
    let mut state = create_test_state();
    Arc::get_mut(&mut state).unwrap().backup_manager =
        Arc::new(BackupManager::new(temp.path().join("backups")));
    let entry = state.backup_manager.create_backup(&game).unwrap();
    std::fs::write(&file, "Changed").unwrap();
    (temp, state, entry, file)
}

#[tokio::test]
async fn backup_handlers_refuse_project_conflicts_before_mutation() {
    let (_temp, state, backup, file) = fixture();
    let exclusive = try_project_operation(&state).unwrap();
    let restored = restore_backup(State(state.clone()), AxumPath(backup.id.clone())).await;
    assert_eq!(restored.unwrap_err().0, StatusCode::CONFLICT);
    let deleted = delete_backup(State(state.clone()), AxumPath(backup.id.clone())).await;
    assert_eq!(deleted.unwrap_err().0, StatusCode::CONFLICT);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Changed");
    assert!(backup.path.exists());
    drop(exclusive);
    let report = restore_backup(State(state.clone()), AxumPath(backup.id.clone()))
        .await
        .unwrap()
        .0;
    assert_eq!(report.replaced, vec![PathBuf::from("story.html")]);
    assert!(report.removed.is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Original");
    assert_eq!(
        delete_backup(State(state.clone()), AxumPath(backup.id))
            .await
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert!(!backup.path.exists());
    assert!(try_project_operation(&state).is_ok());
}

#[test]
fn backup_handlers_keep_exclusion_after_disconnected_caller() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_temp, state, backup, file) = fixture();
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (started, entered) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = wait.recv_timeout(std::time::Duration::from_secs(15));
        });
        entered.await.unwrap();
        let pending_state = state.clone();
        let caller =
            tokio::spawn(
                async move { restore_backup(State(pending_state), AxumPath(backup.id)).await },
            );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while state.project_exclusive.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(try_project_operation(&state).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Changed");
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state.project_exclusive.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Original");
        assert!(try_project_operation(&state).is_ok());
    });
}

#[tokio::test]
async fn restore_backup_reports_cross_process_game_lock_conflict() {
    let (_temp, state, backup, file) = fixture();
    let _game_lock = locust_core::patch::GameLock::acquire(file.parent().unwrap()).unwrap();
    let error = restore_backup(State(state), AxumPath(backup.id))
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::CONFLICT);
    assert_eq!(std::fs::read_to_string(file).unwrap(), "Changed");
}
