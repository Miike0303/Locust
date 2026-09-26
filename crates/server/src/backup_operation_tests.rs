use super::*;

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
    assert_eq!(
        restore_backup(State(state.clone()), AxumPath(backup.id.clone()))
            .await
            .unwrap(),
        StatusCode::OK
    );
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
