use super::*;
use crate::{create_router, create_test_state, test_server, JobState};
use axum::{routing::get, Router};
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

type JobStream = JobSubscription;
type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn fixture_stream(
    rx: broadcast::Receiver<serde_json::Value>,
    snapshot: Vec<serde_json::Value>,
) -> JobStream {
    JobSubscription {
        rx,
        replay: Arc::new(std::sync::Mutex::new(snapshot.clone())),
        snapshot,
        include_sequence: true,
    }
}

// Exercise the real upgraded handler, with a gate to put inbound data on the
// wire before replay and a completion signal independent of TCP buffering.
async fn connect_handler(
    found: Option<JobStream>,
) -> (
    ClientSocket,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    test_server::TestServer,
) {
    let (start, ready) = oneshot::channel();
    let (finished, done) = oneshot::channel();
    let input = Arc::new(std::sync::Mutex::new(Some((found, ready, finished))));
    let app = Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let (found, ready, finished) = input.lock().unwrap().take().unwrap();
            async move {
                ws.on_upgrade(move |socket| async move {
                    ready.await.unwrap();
                    handle_job_ws(
                        socket,
                        found,
                        serde_json::json!({"type": "error", "message": "job not found"}),
                    )
                    .await;
                    let _ = finished.send(());
                })
            }
        }),
    );
    let (url, server) = test_server::start_test_server(app).await;
    let (client, _) =
        tokio_tungstenite::connect_async(format!("{}/ws", url.replacen("http", "ws", 1)))
            .await
            .unwrap();
    (client, start, done, server)
}

fn replay_frames(terminal: &str) -> Vec<serde_json::Value> {
    let mut frames: Vec<_> = (0..32)
        .map(|i| serde_json::json!({"type": "progress", "index": i, "data": "x".repeat(4096)}))
        .collect();
    frames.push(serde_json::json!({"type": terminal}));
    frames
}

#[tokio::test]
async fn finished_job_replay_reaches_slow_reader() {
    for terminal in ["done", "error", "completed", "failed"] {
        let expected = replay_frames(terminal);
        let (tx, rx) = broadcast::channel(8);
        let (mut client, start, mut done, server) =
            connect_handler(Some(fixture_stream(rx, expected.clone()))).await;
        // Pending inbound traffic must be drained before dropping the socket.
        client
            .send(ClientMessage::Ping(vec![1, 2, 3]))
            .await
            .unwrap();
        start.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut actual = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match client
                    .next()
                    .await
                    .expect("replay ended before Close")
                    .unwrap()
                {
                    ClientMessage::Text(text) => {
                        actual.push(serde_json::from_str::<serde_json::Value>(&text).unwrap());
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    ClientMessage::Close(frame) => {
                        assert_eq!(frame, None, "preserve the existing empty Close frame");
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("slow replay timed out");
        assert_eq!(actual, expected, "all replay frames must arrive in order");
        assert!(
            matches!(done.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "handler dropped TCP before receiving the client's Close reply"
        );
        // Reading Close queued the reply; flushing actually sends it.
        client.flush().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), done)
            .await
            .expect("handler did not finish after Close reply")
            .unwrap();
        drop(tx);
        server.shutdown().await;
    }
}

#[tokio::test]
async fn job_ws_close_without_reply_is_bounded() {
    for exit in ["replay", "live", "broadcast_closed", "not_found"] {
        let (tx, rx) = broadcast::channel(8);
        let expected = serde_json::json!({"type": "done"});
        let found = match exit {
            "not_found" => None,
            "replay" => Some(fixture_stream(rx, vec![expected.clone()])),
            _ => Some(fixture_stream(rx, vec![])),
        };
        let (mut client, start, mut done, server) = connect_handler(found).await;
        // A non-Close message cannot count as an acknowledgement.
        client
            .send(ClientMessage::Text("still here".into()))
            .await
            .unwrap();
        let began = Instant::now();
        start.send(()).unwrap();
        if exit == "live" {
            tx.send(expected.clone()).unwrap();
        }
        drop(tx);
        let mut actual = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match client.next().await.expect("expected Close").unwrap() {
                    ClientMessage::Text(text) => {
                        actual.push(serde_json::from_str::<serde_json::Value>(&text).unwrap());
                    }
                    ClientMessage::Close(_) => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        match exit {
            "replay" | "live" => assert_eq!(actual, vec![expected]),
            "not_found" => assert_eq!(
                actual,
                vec![serde_json::json!({"type": "error", "message": "job not found"})]
            ),
            _ => assert!(actual.is_empty()),
        }
        // Do not flush the queued Close reply. Keep TCP open throughout.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut done)
                .await
                .is_err(),
            "{exit}: handler must wait for a Close reply"
        );
        tokio::time::timeout(Duration::from_secs(3), done)
            .await
            .unwrap_or_else(|_| panic!("{exit}: missing Close reply kept handler alive"))
            .unwrap();
        assert!(began.elapsed() >= Duration::from_millis(1900), "{exit}");
        assert!(began.elapsed() < Duration::from_secs(3), "{exit}");
        drop(client);
        server.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual stress test: 1024 finished-job replays, 64 concurrent clients"]
async fn job_ws_finished_replay_stress() {
    stress_finished_replays(false).await;
}

#[tokio::test]
async fn job_ws_disconnected_client_does_not_stall_sends() {
    for live in [false, true] {
        let events = vec![
            serde_json::json!({"type": "progress", "data": "x".repeat(8 * 1024 * 1024)}),
            serde_json::json!({"type": "done"}),
        ];
        let (tx, rx) = broadcast::channel(8);
        let replay = if live { vec![] } else { events.clone() };
        let (client, start, done, server) = connect_handler(Some(fixture_stream(rx, replay))).await;
        if live {
            for event in events {
                tx.send(event).unwrap();
            }
        }
        // Closing TCP without a WebSocket Close makes the replay/live write
        // fail. Neither the write error nor its cleanup may strand the handler.
        drop(client);
        start.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), done)
            .await
            .expect("disconnected client's send error kept handler alive")
            .unwrap();
        drop(tx);
        server.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual stress test: 1024 replays with pending inbound Ping frames"]
async fn job_ws_finished_replay_with_pending_ping_stress() {
    stress_finished_replays(true).await;
}

async fn stress_finished_replays(send_ping: bool) {
    let state = create_test_state();
    let expected = replay_frames("done");
    let (tx, _) = broadcast::channel(8);
    state.active_jobs.insert(
        "finished".into(),
        JobState {
            abort_handle: tokio::spawn(async {}).abort_handle(),
            progress_tx: tx,
            replay: Arc::new(std::sync::Mutex::new(expected.clone())),
            cancel: tokio_util::sync::CancellationToken::new(),
            kind: JobKind::Patch,
            patch_game_key: None,
        },
    );
    let (url, server) = test_server::start_test_server(create_router(state)).await;
    let url = format!("{}/api/patch/ws/finished", url.replacen("http", "ws", 1));
    let mut before_terminal = 0;
    let mut after_terminal = 0;
    let mut resets = 0;
    let mut incomplete = 0;
    for _ in 0..16 {
        let mut clients = tokio::task::JoinSet::new();
        for _ in 0..64 {
            let url = url.clone();
            let expected = expected.clone();
            clients.spawn(async move {
                let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
                let mut frames = Vec::new();
                let mut failure = None;
                let result = tokio::time::timeout(Duration::from_secs(10), async {
                    if send_ping {
                        if let Err(error) = ws.send(ClientMessage::Ping(vec![1, 2, 3])).await {
                            failure = Some(error);
                        }
                    }
                    // Let concurrent replay and close writes accumulate first.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    while let Some(message) = ws.next().await {
                        match message {
                            Ok(ClientMessage::Text(text)) => frames
                                .push(serde_json::from_str::<serde_json::Value>(&text).unwrap()),
                            Ok(ClientMessage::Close(_)) => {
                                if let Err(error) = ws.flush().await {
                                    failure = Some(error);
                                }
                                break;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                failure = Some(error);
                                break;
                            }
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await;
                let terminal = frames.iter().any(job_event_is_terminal);
                let reset = failure.as_ref().is_some_and(|e| {
                    matches!(e, tokio_tungstenite::tungstenite::Error::Io(io)
                        if io.kind() == std::io::ErrorKind::ConnectionReset)
                });
                (
                    failure.is_some() || result.is_err(),
                    terminal,
                    reset,
                    frames == expected,
                )
            });
        }
        while let Some(result) = clients.join_next().await {
            let (failed, terminal, reset, complete) = result.unwrap();
            before_terminal += usize::from(failed && !terminal);
            after_terminal += usize::from(failed && terminal);
            resets += usize::from(reset);
            incomplete += usize::from(!complete);
        }
    }
    eprintln!(
        "1024 replays (send_ping={send_ping}): errors before terminal={before_terminal}, after terminal={after_terminal}, ConnectionReset={resets}, incomplete={incomplete}"
    );
    server.shutdown().await;
    assert_eq!(before_terminal, 0);
    assert_eq!(incomplete, 0);
}

const EXPECTED_REPLAY_LIMIT: usize = 256;
type Replay = Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

fn emit(tx: &broadcast::Sender<serde_json::Value>, replay: &Replay, event: serde_json::Value) {
    publish_job_event(tx, replay, event);
}

fn payload(mut event: serde_json::Value) -> serde_json::Value {
    event.as_object_mut().unwrap().remove("seq");
    event
}

async fn read_to_close(client: &mut ClientSocket) -> Vec<serde_json::Value> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(message) = client.next().await {
            match message.unwrap() {
                ClientMessage::Text(text) => events.push(serde_json::from_str(&text).unwrap()),
                ClientMessage::Close(_) => {
                    client.flush().await.unwrap();
                    return events;
                }
                _ => {}
            }
        }
        panic!("job socket ended without Close");
    })
    .await
    .expect("job socket did not terminate")
}

#[test]
fn replay_bounds_100k_progress_events() {
    let (tx, _) = broadcast::channel(8);
    let replay = Replay::default();
    let started = serde_json::json!({"type":"started", "total":100000, "job_id":"bounds"});
    let mut before_bytes = 2 + started.to_string().len();
    emit(&tx, &replay, started);
    for completed in 1..=100_000 {
        let event = serde_json::json!({"type":"batch_completed", "completed":completed,
            "total":100000, "cost_so_far":0.0, "cost_is_complete":true, "language":"es"});
        before_bytes += 1 + event.to_string().len();
        emit(&tx, &replay, event);
    }
    let log = replay.lock().unwrap();
    eprintln!(
        "100k progress: before length=100001 bytes={before_bytes}; after length={} bytes={}",
        log.len(),
        serde_json::to_vec(&*log).unwrap().len()
    );
    assert!(
        log.len() <= EXPECTED_REPLAY_LIMIT,
        "replay retained {} entries",
        log.len()
    );
    assert_eq!(log.last().unwrap()["completed"], 100000);
    assert_eq!(log.last().unwrap()["seq"], 100001);
    assert_eq!(
        log.iter()
            .filter(|event| event["type"] == "batch_completed")
            .count(),
        1
    );
}

#[tokio::test]
async fn replay_bounds_mid_job_reconnect_keeps_required_events_and_latest_progress() {
    let (tx, _) = broadcast::channel(256);
    let replay = Replay::default();
    let initial = serde_json::json!({"type":"started", "total":100, "job_id":"reconnect"});
    emit(&tx, &replay, initial.clone());
    let information = vec![
        serde_json::json!({"type":"string_translated", "entry_id":"row", "translation":"Hola"}),
        serde_json::json!({"type":"validation_failed", "issues":[]}),
        serde_json::json!({"type":"paused"}),
        serde_json::json!({"type":"resumed"}),
        serde_json::json!({"type":"batch_failed", "entry_id":null, "error":"retry"}),
        serde_json::json!({"type":"provider_switched", "provider_id":"next",
            "provider_name":"Next", "remaining_pending":50}),
    ];
    for event in &information {
        emit(&tx, &replay, event.clone());
    }
    let latest = serde_json::json!({"type":"batch_completed", "completed":99, "total":100,
        "cost_so_far":1.5, "cost_is_complete":true, "language":"es"});
    for completed in 1..99 {
        let mut event = latest.clone();
        event["completed"] = completed.into();
        emit(&tx, &replay, event);
    }
    emit(&tx, &replay, latest.clone());
    let rx = tx.subscribe();
    let snapshot = replay.lock().unwrap().clone();
    let (mut client, start, done, server) = connect_handler(Some(JobSubscription {
        rx,
        snapshot,
        replay: replay.clone(),
        include_sequence: true,
    }))
    .await;
    let terminal = serde_json::json!({"type":"completed", "total_translated":100,
        "remaining":0, "total_cost":1.5, "cost_is_complete":true, "duration_secs":1.0});
    emit(&tx, &replay, terminal.clone());
    start.send(()).unwrap();
    let mut expected = vec![initial];
    expected.extend(information);
    expected.extend([latest, terminal]);
    assert_eq!(
        read_to_close(&mut client)
            .await
            .into_iter()
            .map(payload)
            .collect::<Vec<_>>(),
        expected
    );
    done.await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn replay_bounds_subscribe_snapshot_race_has_no_duplicates_or_gaps() {
    let (tx, _) = broadcast::channel(32);
    let replay = Replay::default();
    let started = serde_json::json!({"type":"started", "total":2, "job_id":"race"});
    let repeated = serde_json::json!({"type":"batch_failed", "entry_id":null, "error":"retry"});
    emit(&tx, &replay, started.clone());
    let rx = tx.subscribe();
    // This publication is both queued live and included in the snapshot.
    emit(&tx, &replay, repeated.clone());
    let snapshot = replay.lock().unwrap().clone();
    let (mut client, start, done, server) = connect_handler(Some(JobSubscription {
        rx,
        snapshot,
        replay: replay.clone(),
        include_sequence: true,
    }))
    .await;
    // Equal payload, different publication: must arrive exactly once as well.
    emit(&tx, &replay, repeated.clone());
    let terminal = serde_json::json!({"type":"completed"});
    emit(&tx, &replay, terminal.clone());
    start.send(()).unwrap();
    let events = read_to_close(&mut client).await;
    assert_eq!(
        events.into_iter().map(payload).collect::<Vec<_>>(),
        vec![started, repeated.clone(), repeated, terminal]
    );
    done.await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn replay_bounds_lagged_receiver_recovers_evicted_information_and_latest_state() {
    let (tx, rx) = broadcast::channel(2);
    let replay = Replay::default();
    let (mut client, start, done, server) = connect_handler(Some(JobSubscription {
        rx,
        snapshot: vec![],
        replay: replay.clone(),
        include_sequence: true,
    }))
    .await;
    let started = serde_json::json!({"type":"started", "total":100, "job_id":"lag"});
    let info = serde_json::json!({"type":"provider_switched", "provider_id":"fallback",
        "provider_name":"Fallback", "remaining_pending":100});
    emit(&tx, &replay, started.clone());
    emit(&tx, &replay, info.clone());
    for current in 1..=100 {
        emit(
            &tx,
            &replay,
            serde_json::json!({"type":"progress", "current":current, "total":100}),
        );
    }
    let terminal = serde_json::json!({"type":"done"});
    emit(&tx, &replay, terminal.clone());
    start.send(()).unwrap();
    assert_eq!(
        read_to_close(&mut client)
            .await
            .into_iter()
            .map(payload)
            .collect::<Vec<_>>(),
        vec![
            started,
            info,
            serde_json::json!({"type":"progress", "current":100, "total":100}),
            terminal
        ]
    );
    done.await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn replay_bounds_cancellation_and_worker_race_keeps_first_terminal() {
    for worker_first in [false, true] {
        let (tx, mut rx) = broadcast::channel(8);
        let replay = Replay::default();
        let cancel = serde_json::json!({"type":"failed", "entry_id":null, "error":"cancelled"});
        let worker = serde_json::json!({"type":"completed", "total_translated":1});
        let (first, second) = if worker_first {
            (worker, cancel)
        } else {
            (cancel, worker)
        };
        emit(&tx, &replay, first.clone());
        let gate = Arc::new(std::sync::Barrier::new(3));
        std::thread::scope(|scope| {
            for event in [
                second,
                serde_json::json!({"type":"progress", "current":999}),
            ] {
                let gate = gate.clone();
                let tx = &tx;
                let replay = &replay;
                scope.spawn(move || {
                    gate.wait();
                    emit(tx, replay, event);
                });
            }
            gate.wait();
        });
        assert_eq!(
            replay
                .lock()
                .unwrap()
                .clone()
                .into_iter()
                .map(payload)
                .collect::<Vec<_>>(),
            vec![first.clone()]
        );
        assert_eq!(payload(rx.try_recv().unwrap()), first);
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}

#[test]
fn replay_bounds_informational_overflow_pins_state_and_keeps_recent_tail() {
    let (tx, _) = broadcast::channel(8);
    let replay = Replay::default();
    let state = vec![
        serde_json::json!({"type":"started", "total":100000, "job_id":"initial"}),
        serde_json::json!({"type":"started", "total":50000, "job_id":"fallback"}),
        serde_json::json!({"type":"batch_completed", "completed":42, "total":50000}),
        serde_json::json!({"type":"progress", "current":42, "total":50000}),
        serde_json::json!({"type":"string_translated", "entry_id":"latest", "translation":"Hola"}),
        serde_json::json!({"type":"provider_switched", "provider_id":"fallback"}),
        serde_json::json!({"type":"validation_failed", "issues":[]}),
        serde_json::json!({"type":"paused"}),
        serde_json::json!({"type":"resumed"}),
    ];
    for event in &state {
        emit(&tx, &replay, event.clone());
    }
    for index in 0..1000 {
        emit(
            &tx,
            &replay,
            serde_json::json!({"type":"batch_failed", "index":index, "error":"retry"}),
        );
        assert!(replay.lock().unwrap().len() <= EXPECTED_REPLAY_LIMIT);
    }
    emit(&tx, &replay, serde_json::json!({"type":"completed"}));
    let log = replay.lock().unwrap();
    assert_eq!(log.len(), EXPECTED_REPLAY_LIMIT);
    assert_eq!(EXPECTED_REPLAY_LIMIT, crate::JOB_REPLAY_LIMIT);
    assert!(log
        .windows(2)
        .all(|pair| pair[0]["seq"].as_u64().unwrap() < pair[1]["seq"].as_u64().unwrap()));
    let actual = log.iter().cloned().map(payload).collect::<Vec<_>>();
    assert_eq!(&actual[..state.len()], &state);
    let indices = actual
        .iter()
        .filter(|event| event["type"] == "batch_failed")
        .map(|event| event["index"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        indices,
        (1000 - indices.len() as u64..1000).collect::<Vec<_>>()
    );
    assert_eq!(actual.last().unwrap()["type"], "completed");
}

#[tokio::test]
async fn replay_bounds_concurrent_cancel_and_worker_agree_on_terminal() {
    for _ in 0..32 {
        let state = create_test_state();
        let (tx, mut rx) = broadcast::channel(8);
        let replay = Replay::default();
        state.active_jobs.insert(
            "race".into(),
            JobState {
                abort_handle: tokio::spawn(async {}).abort_handle(),
                progress_tx: tx.clone(),
                replay: replay.clone(),
                cancel: tokio_util::sync::CancellationToken::new(),
                kind: JobKind::Translate,
                patch_game_key: None,
            },
        );
        let gate = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                gate.wait();
                assert_eq!(cancel_job(&state, "race").unwrap(), StatusCode::OK);
            });
            scope.spawn(|| {
                gate.wait();
                emit(&tx, &replay, serde_json::json!({"type":"completed"}));
            });
            gate.wait();
        });
        let log = replay.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert!(job_event_is_terminal(&log[0]));
        assert_eq!(rx.try_recv().unwrap(), log[0]);
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn replay_bounds_lag_after_delivered_prefix_does_not_repeat_it() {
    let (tx, _) = broadcast::channel(2);
    let replay = Replay::default();
    let initial = serde_json::json!({"type":"started", "total":100, "job_id":"prefix"});
    let repeated = serde_json::json!({"type":"batch_failed", "entry_id":null, "error":"retry"});
    emit(&tx, &replay, initial.clone());
    emit(&tx, &replay, repeated.clone());
    let subscription = JobSubscription::subscribe(&tx, &replay, JobKind::Translate);
    let (mut client, start, done, server) = connect_handler(Some(subscription)).await;
    start.send(()).unwrap();
    let mut actual = Vec::new();
    for _ in 0..2 {
        let frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let ClientMessage::Text(text) = frame else {
            panic!("expected replay prefix")
        };
        actual.push(serde_json::from_str::<serde_json::Value>(&text).unwrap());
    }
    // No await in this burst: the real handler must observe an overrun before
    // it can consume the next live frame, despite having delivered a prefix.
    emit(&tx, &replay, repeated.clone());
    for current in 1..=100 {
        emit(
            &tx,
            &replay,
            serde_json::json!({"type":"progress", "current":current, "total":100}),
        );
    }
    let terminal = serde_json::json!({"type":"done"});
    emit(&tx, &replay, terminal.clone());
    actual.extend(read_to_close(&mut client).await);
    assert!(actual
        .windows(2)
        .all(|pair| pair[0]["seq"].as_u64().unwrap() < pair[1]["seq"].as_u64().unwrap()));
    assert_eq!(
        actual.into_iter().map(payload).collect::<Vec<_>>(),
        vec![
            initial,
            repeated.clone(),
            repeated,
            serde_json::json!({"type":"progress", "current":100, "total":100}),
            terminal,
        ]
    );
    done.await.unwrap();
    server.shutdown().await;
}

#[test]
fn replay_bounds_100k_interleaved_row_and_progress_events() {
    let (tx, _) = broadcast::channel(8);
    let replay = Replay::default();
    emit(
        &tx,
        &replay,
        serde_json::json!({"type":"started", "total":100000, "job_id":"rows"}),
    );
    let last_failure = serde_json::json!({"type":"batch_failed", "entry_id":null, "error":"retry"});
    emit(&tx, &replay, last_failure.clone());
    for current in 1..=100_000 {
        emit(
            &tx,
            &replay,
            serde_json::json!({"type":"string_translated", "entry_id":current.to_string(), "translation":"Hola"}),
        );
        emit(
            &tx,
            &replay,
            serde_json::json!({"type":"batch_completed", "completed":current, "total":100000}),
        );
        assert!(replay.lock().unwrap().len() <= crate::JOB_REPLAY_LIMIT);
    }
    let log = replay.lock().unwrap();
    assert_eq!(log.len(), crate::JOB_REPLAY_LIMIT);
    assert_eq!(log[0]["type"], "started");
    assert_eq!(payload(log[1].clone()), last_failure);
    assert_eq!(log[log.len() - 2]["entry_id"], "100000");
    assert_eq!(log.last().unwrap()["completed"], 100000);
    assert_eq!(log.last().unwrap()["seq"], 200002);
}
