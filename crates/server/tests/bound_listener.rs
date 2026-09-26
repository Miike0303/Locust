#[tokio::test]
async fn reserved_desktop_socket_serves_without_rebinding_or_startup_sleep() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    // A competing process cannot claim the selected endpoint in the handoff.
    assert!(std::net::TcpListener::bind(address).is_err());
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let state = locust_server::create_test_state();
    let server = tokio::spawn(locust_server::start_server_with_listener(state, listener));
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("http://{address}/health"))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    server.abort();
    let _ = server.await;
    assert!(status.is_success());
    assert_eq!(body["status"], "ok");
}
