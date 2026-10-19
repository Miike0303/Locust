pub(crate) async fn cmd_server(port: u16) -> anyhow::Result<()> {
    let state = locust_server::create_app_state();
    println!(
        "Starting Project Locust server on http://localhost:{}",
        port
    );
    println!("Press Ctrl+C to stop");
    locust_server::start_server(state, port).await?;
    Ok(())
}
