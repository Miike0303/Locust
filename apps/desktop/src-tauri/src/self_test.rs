//! Headless executable acceptance: no Tauri runtime or display initialization.

use anyhow::{ensure, Context};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

#[derive(Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Report {
    ok: bool,
    version: &'static str,
    port: Option<u16>,
    checks: Vec<Check>,
}

impl Report {
    fn new() -> Self {
        Self {
            ok: false,
            version: env!("CARGO_PKG_VERSION"),
            port: None,
            checks: Vec::new(),
        }
    }

    fn check<T>(
        &mut self,
        name: &'static str,
        operation: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let result = operation();
        self.checks.push(Check {
            name,
            ok: result.is_ok(),
            error: result.as_ref().err().map(|error| format!("{error:#}")),
        });
        result
    }
}

fn check_health(health: &Value) -> anyhow::Result<()> {
    ensure!(
        health["status"] == "ok",
        "Health status is not ok: {health}"
    );
    ensure!(
        health["version"] == env!("CARGO_PKG_VERSION"),
        "Health version does not match the desktop: {health}"
    );
    Ok(())
}

fn parse_response(response: &[u8]) -> anyhow::Result<Value> {
    ensure!(
        response.len() as u64 <= MAX_RESPONSE_BYTES,
        "HTTP response exceeds the self-test limit"
    );
    let text = std::str::from_utf8(response).context("HTTP response is not UTF-8")?;
    let (headers, body) = text
        .split_once("\r\n\r\n")
        .context("Incomplete HTTP response")?;
    let mut status = headers
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    ensure!(
        matches!(status.next(), Some("HTTP/1.0" | "HTTP/1.1")) && status.next() == Some("200"),
        "HTTP request failed: {}",
        headers.lines().next().unwrap_or_default()
    );
    serde_json::from_str(body).context("HTTP response is not valid JSON")
}

fn get_json(port: u16, route: &str) -> anyhow::Result<Value> {
    // A direct socket avoids proxy environment variables and new dependencies.
    // HTTP/1.0 + close gives a bounded, unchunked JSON response from Axum.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
            let request = format!(
                "GET {route} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(request.as_bytes()).await?;
            let mut response = Vec::new();
            stream
                .take(MAX_RESPONSE_BYTES + 1)
                .read_to_end(&mut response)
                .await?;
            parse_response(&response)
        })
        .await
        .context("Self-test HTTP request timed out")?
    })
    .with_context(|| format!("GET {route} on 127.0.0.1:{port}"))
}

fn check_backend(backend: crate::Backend, report: &mut Report) -> anyhow::Result<()> {
    report.port = Some(backend.port);
    let result = (|| {
        report.check("health", || {
            check_health(&get_json(backend.port, "/health")?)
        })?;
        for (name, route) in [("formats", "/api/formats"), ("providers", "/api/providers")] {
            report.check(name, || {
                let value = get_json(backend.port, route)?;
                ensure!(
                    value.as_array().is_some_and(|items| !items.is_empty()),
                    "{route} did not return an initialized registry"
                );
                Ok(())
            })?;
        }
        Ok(())
    })();
    // Always release the listener and join, including after a failed probe.
    let shutdown = report.check("shutdown", || backend.shutdown());
    result.and(shutdown)
}

pub(crate) fn run() -> i32 {
    let mut report = Report::new();
    let result = report
        .check("app_state", || {
            ensure!(
                std::env::var_os("LOCUST_DATA_DIR").is_some_and(|path| !path.is_empty()),
                "--self-test requires an isolated LOCUST_DATA_DIR"
            );
            crate::start_backend()
        })
        .and_then(|backend| check_backend(backend, &mut report));
    report.ok = result.is_ok();
    if let Err(error) = &result {
        eprintln!("Locust could not start.\n\n{error:#}");
    }
    println!(
        "{}",
        serde_json::to_string(&report).expect("serializable self-test report")
    );
    i32::from(result.is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn health_requires_ok_status_and_the_desktop_version() {
        check_health(&json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION")
        }))
        .unwrap();

        for health in [
            json!({"status": "failed", "version": env!("CARGO_PKG_VERSION")}),
            json!({"status": "ok", "version": "wrong-version"}),
            json!({}),
            json!(null),
        ] {
            assert!(check_health(&health).is_err(), "accepted {health}");
        }
    }

    #[test]
    fn http_response_requires_success_and_bounded_json() {
        let good = b"HTTP/1.0 200 OK\r\nContent-Length: 15\r\n\r\n{\"status\":\"ok\"}";
        assert_eq!(parse_response(good).unwrap(), json!({"status": "ok"}));
        for bad in [
            b"HTTP/1.0 503 Unavailable\r\n\r\n{}".as_slice(),
            b"HTTP/1.1 200 OK\r\n\r\nnot json",
            b"incomplete",
        ] {
            assert!(parse_response(bad).is_err());
        }
        assert!(parse_response(&vec![b' '; MAX_RESPONSE_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn real_backend_is_probed_and_stopped_without_tauri() {
        let backend = crate::start_backend_with_state(locust_server::create_test_state()).unwrap();
        let port = backend.port;
        let mut report = Report::new();
        check_backend(backend, &mut report).unwrap();
        assert_eq!(report.port, Some(port));
        assert_eq!(
            report
                .checks
                .iter()
                .map(|check| check.name)
                .collect::<Vec<_>>(),
            ["health", "formats", "providers", "shutdown"]
        );
        assert!(report.checks.iter().all(|check| check.ok));
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    }
}
