use tokio::{sync::oneshot, task::JoinHandle};

pub struct TestServer {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl TestServer {
    pub async fn shutdown(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        // Axum also waits for connection tasks to release their AppState clones.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.task.take().unwrap(),
        )
        .await
        .expect("test server shutdown timed out")
        .unwrap();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

pub async fn start_test_server(app: axum::Router) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    (
        url,
        TestServer {
            shutdown: Some(shutdown),
            task: Some(task),
        },
    )
}
