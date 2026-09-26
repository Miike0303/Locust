use super::*;

#[test]
fn terminal_publication_is_final_for_broadcast_and_replay() {
    let (tx, mut rx) = broadcast::channel(8);
    let replay = std::sync::Mutex::new(Vec::new());
    let cancelled = serde_json::json!({"type":"failed","error":"cancelled"});
    publish_job_event(&tx, &replay, cancelled.clone());
    publish_job_event(&tx, &replay, serde_json::json!({"type":"completed"}));
    publish_job_event(&tx, &replay, serde_json::json!({"type":"progress"}));
    assert_eq!(*replay.lock().unwrap(), vec![cancelled.clone()]);
    assert_eq!(rx.try_recv().unwrap(), cancelled);
    assert!(matches!(
        rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
}

#[test]
fn cancellation_drains_an_already_queued_sqlite_write_before_unlocking() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let state = create_test_state();
        state
            .db
            .save_entries(&[StringEntry::new("row", "Original", "story.html".into())])
            .unwrap();
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            // Disconnect on assertion failure also releases the blocking pool.
            let _ = blocked.recv_timeout(std::time::Duration::from_secs(10));
        });
        entered_rx.await.unwrap();
        let guard = try_project_operation(&state).unwrap();
        let db = state.db.clone();
        let (queued_tx, queued_rx) = tokio::sync::oneshot::channel();
        let mut worker = tokio::spawn(async move {
            let _guard = guard;
            // This poll continues directly into save_translation, which queues
            // the blocking write before yielding to its JoinHandle.
            queued_tx.send(()).unwrap();
            db.save_translation("row", "Queued translation", "local-test")
                .await
                .unwrap();
        });
        queued_rx.await.unwrap();
        let (tx, _) = broadcast::channel(8);
        state.active_jobs.insert(
            "queued".into(),
            JobState {
                abort_handle: worker.abort_handle(),
                progress_tx: tx,
                replay: Arc::new(std::sync::Mutex::new(Vec::new())),
                cancel: tokio_util::sync::CancellationToken::new(),
                kind: JobKind::Translate,
                patch_game_key: None,
            },
        );
        cancel_translation_job(&state, "queued").unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut worker)
                .await
                .is_err(),
            "cancel must not abort a worker awaiting an unabortable SQLite write"
        );
        assert!(try_project_operation(&state).is_err());
        assert_eq!(
            state.db.get_entry("row").unwrap().unwrap().translation,
            None
        );
        release.send(()).unwrap();
        blocker.await.unwrap();
        worker.await.unwrap();
        assert_eq!(
            state
                .db
                .get_entry("row")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Queued translation")
        );
        assert_eq!(state.project_exclusive.load(Ordering::SeqCst), 0);
        assert!(try_project_operation(&state).is_ok());
    });
}
