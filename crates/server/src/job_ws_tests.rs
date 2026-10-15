use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

type JobStream = (
    broadcast::Receiver<serde_json::Value>,
    Vec<serde_json::Value>,
);
type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
            connect_handler(Some((rx, expected.clone()))).await;
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
            "replay" => Some((rx, vec![expected.clone()])),
            _ => Some((rx, vec![])),
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
        let (client, start, done, server) = connect_handler(Some((rx, replay))).await;
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
