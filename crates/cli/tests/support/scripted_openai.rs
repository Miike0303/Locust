//! A bounded, loopback-only OpenAI-compatible endpoint. Answers are keyed by
//! language pair and masked source, never request order or database row order.
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const PROVIDER: &str = "pipeline-scripted";
pub const DUMMY_KEY: &str = "pipeline-dummy-not-a-credential";
pub const MODEL: &str = "scripted-fixture";

const SOCKET_POLL: Duration = Duration::from_millis(50);
const CONNECTION_LIMIT: Duration = Duration::from_secs(2);

/// One budget for the entire accepted connection, not a fresh timeout per read
/// or write. Short socket waits make Drop cancellation visible even without IO.
struct IoBudget<'a> {
    stop: &'a AtomicBool,
    deadline: Instant,
}

impl IoBudget<'_> {
    fn check(&self) -> Result<(), String> {
        if self.stop.load(Ordering::Acquire) {
            return Err("endpoint shutdown requested".into());
        }
        if Instant::now() >= self.deadline {
            return Err("connection deadline exceeded".into());
        }
        Ok(())
    }
}

fn retryable_io(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestKey {
    pub source_lang: String,
    pub target_lang: String,
    pub source: String,
}

impl RequestKey {
    pub fn new(source_lang: &str, target_lang: &str, source: &str) -> Self {
        Self {
            source_lang: source_lang.into(),
            target_lang: target_lang.into(),
            source: source.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Answer {
    Text(String),
    Missing,
}

#[derive(Default)]
struct Evidence {
    requests: Vec<RequestKey>,
    errors: Vec<String>,
}

pub struct ScriptedOpenAi {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    evidence: Arc<Mutex<Evidence>>,
    accepted: Arc<(Mutex<usize>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl ScriptedOpenAi {
    pub fn start(answers: BTreeMap<RequestKey, Answer>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let evidence = Arc::new(Mutex::new(Evidence::default()));
        let accepted = Arc::new((Mutex::new(0), Condvar::new()));
        let thread_accepted = accepted.clone();
        let thread_stop = stop.clone();
        let thread_evidence = evidence.clone();
        let worker = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, peer)) => {
                        assert!(peer.ip().is_loopback());
                        {
                            let (count, ready) = &*thread_accepted;
                            *count.lock().unwrap() += 1;
                            ready.notify_all();
                        }
                        let result = serve(&mut stream, &answers, &thread_evidence, &thread_stop);
                        if let Err(error) = result {
                            thread_evidence.lock().unwrap().errors.push(error);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => {
                        thread_evidence
                            .lock()
                            .unwrap()
                            .errors
                            .push(error.to_string());
                        break;
                    }
                }
            }
        });
        Self {
            address,
            stop,
            evidence,
            accepted,
            worker: Some(worker),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<RequestKey> {
        let evidence = self.evidence.lock().unwrap();
        assert!(evidence.errors.is_empty(), "{:?}", evidence.errors);
        evidence.requests.clone()
    }
}

impl Drop for ScriptedOpenAi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("scripted endpoint thread panicked");
        }
    }
}

fn serve(
    stream: &mut TcpStream,
    answers: &BTreeMap<RequestKey, Answer>,
    evidence: &Mutex<Evidence>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let budget = IoBudget {
        stop,
        deadline: Instant::now() + CONNECTION_LIMIT,
    };
    budget.check()?;
    // Windows can inherit the listener's nonblocking mode on accepted sockets.
    stream
        .set_nonblocking(false)
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(SOCKET_POLL))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(SOCKET_POLL))
        .map_err(|error| error.to_string())?;
    let result = read_request(stream, &budget).and_then(|body| {
        budget.check()?;
        answer_request(body, answers, evidence)
    });
    // Do not attempt an error response after cancellation or deadline expiry.
    budget.check()?;
    let (status, body) = match &result {
        Ok(body) => ("200 OK", body.clone()),
        Err(error) => ("400 Bad Request", json!({"error": error})),
    };
    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_with_budget(stream, headers.as_bytes(), &budget)?;
    write_with_budget(stream, &body, &budget)?;
    result.map(|_| ())
}

fn write_with_budget(
    stream: &mut impl Write,
    mut bytes: &[u8],
    budget: &IoBudget<'_>,
) -> Result<(), String> {
    while !bytes.is_empty() {
        budget.check()?;
        match stream.write(bytes) {
            Ok(0) => return Err("fixture response write made no progress".into()),
            Ok(size) => bytes = &bytes[size..],
            Err(error) if retryable_io(&error) => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    budget.check()
}

fn read_request(stream: &mut TcpStream, budget: &IoBudget<'_>) -> Result<Value, String> {
    const LIMIT: usize = 64 * 1024;
    let mut bytes = Vec::new();
    let (body_start, length) = loop {
        budget.check()?;
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..index]).map_err(|e| e.to_string())?;
            if headers.lines().next() != Some("POST /v1/chat/completions HTTP/1.1") {
                return Err(format!("unexpected endpoint: {headers}"));
            }
            let mut fields = BTreeMap::new();
            for line in headers.lines().skip(1) {
                let (name, value) = line.split_once(':').ok_or("malformed HTTP header")?;
                fields.insert(name.to_ascii_lowercase(), value.trim());
            }
            let authorization = format!("Bearer {DUMMY_KEY}");
            if fields.get("authorization") != Some(&authorization.as_str()) {
                return Err("expected only the fixture dummy key".into());
            }
            let length = fields
                .get("content-length")
                .ok_or("missing Content-Length")?
                .parse::<usize>()
                .map_err(|e| e.to_string())?;
            if index + 4 + length > LIMIT {
                return Err("fixture request too large".into());
            }
            break (index + 4, length);
        }
        read_chunk(stream, &mut bytes, LIMIT, budget)?;
    };
    while bytes.len() < body_start + length {
        read_chunk(stream, &mut bytes, LIMIT, budget)?;
    }
    serde_json::from_slice(&bytes[body_start..body_start + length]).map_err(|e| e.to_string())
}

fn read_chunk(
    stream: &mut TcpStream,
    bytes: &mut Vec<u8>,
    limit: usize,
    budget: &IoBudget<'_>,
) -> Result<(), String> {
    let mut chunk = [0; 4096];
    let size = loop {
        budget.check()?;
        match stream.read(&mut chunk) {
            Ok(size) => break size,
            Err(error) if retryable_io(&error) => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    budget.check()?;
    if size == 0 || bytes.len() + size > limit {
        return Err("incomplete or oversized fixture request".into());
    }
    bytes.extend_from_slice(&chunk[..size]);
    Ok(())
}

fn answer_request(
    body: Value,
    answers: &BTreeMap<RequestKey, Answer>,
    evidence: &Mutex<Evidence>,
) -> Result<Value, String> {
    if body["model"] != MODEL {
        return Err(format!("unexpected model: {}", body["model"]));
    }
    let messages = body["messages"].as_array().ok_or("missing messages")?;
    if messages.len() != 2 || messages[0]["role"] != "system" || messages[1]["role"] != "user" {
        return Err("expected system and user messages".into());
    }
    let system = messages[0]["content"].as_str().ok_or("missing prompt")?;
    let pair = [("ja", "en"), ("en", "es")]
        .into_iter()
        .find(|(source, target)| system.contains(&format!("from {source} to {target}.")))
        .ok_or("unexpected language stage")?;
    if !system.contains("Preserve all placeholder tokens") {
        return Err("missing placeholder instruction".into());
    }
    let records: Vec<Value> = serde_json::from_str(
        messages[1]["content"]
            .as_str()
            .ok_or("missing input records")?,
    )
    .map_err(|e| e.to_string())?;
    if records.len() != 1 || records[0]["index"] != 0 {
        return Err("expected batch-size 1".into());
    }
    let key = RequestKey::new(
        pair.0,
        pair.1,
        records[0]["source"].as_str().ok_or("missing source")?,
    );
    let answer = answers
        .get(&key)
        .ok_or_else(|| format!("unscripted request: {key:?}"))?;
    evidence.lock().unwrap().requests.push(key);
    let translated = match answer {
        Answer::Text(text) => vec![text.clone()],
        Answer::Missing => Vec::new(),
    };
    Ok(json!({
        "choices": [{"message": {"content": serde_json::to_string(&translated).unwrap()}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    use std::sync::mpsc;

    fn await_accept(server: &ScriptedOpenAi) {
        let (count, ready) = &*server.accepted;
        let (count, _) = ready
            .wait_timeout_while(count.lock().unwrap(), Duration::from_secs(5), |count| {
                *count == 0
            })
            .unwrap();
        assert!(*count > 0, "endpoint never accepted the test connection");
    }

    #[test]
    fn endpoint_drop_cancels_stalled_accepted_connections() {
        for incomplete_body in [false, true] {
            let server = ScriptedOpenAi::start(BTreeMap::new());
            let mut client = TcpStream::connect(server.address).unwrap();
            client
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            if incomplete_body {
                write!(client, "POST /v1/chat/completions HTTP/1.1\r\nAuthorization: Bearer {DUMMY_KEY}\r\nContent-Length: 2\r\n\r\n[").unwrap();
            }
            await_accept(&server);
            let (done, completion) = mpsc::channel();
            let shutdown = thread::spawn(move || {
                let start = Instant::now();
                drop(server);
                done.send(start.elapsed()).unwrap();
            });
            let result = completion.recv_timeout(Duration::from_secs(1));
            // Always release the old implementation before asserting RED. This
            // regression must never strand a worker or wait for its 5s timeout.
            let _ = client.shutdown(Shutdown::Both);
            shutdown.join().unwrap();
            assert!(
                result.is_ok(),
                "Drop needed peer closure while reading {}: {result:?}",
                if incomplete_body { "body" } else { "headers" }
            );
        }
    }

    #[test]
    fn response_writes_observe_cancellation_and_absolute_deadline() {
        let stop = AtomicBool::new(false);
        let mut budget = IoBudget {
            stop: &stop,
            deadline: Instant::now() + CONNECTION_LIMIT,
        };
        // A progressing writer triggers cancellation after its first partial
        // write. The next iteration must not reset or bypass the shared budget.
        struct CancelAfterFirstWrite<'a> {
            stop: &'a AtomicBool,
            calls: usize,
        }
        impl Write for CancelAfterFirstWrite<'_> {
            fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                self.stop.store(true, Ordering::Release);
                Ok(1)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CancelAfterFirstWrite {
            stop: &stop,
            calls: 0,
        };
        assert_eq!(
            write_with_budget(&mut writer, b"reply", &budget),
            Err("endpoint shutdown requested".into())
        );
        assert_eq!(writer.calls, 1);
        stop.store(false, Ordering::Release);
        budget.deadline = Instant::now();
        assert_eq!(
            write_with_budget(&mut writer, b"reply", &budget),
            Err("connection deadline exceeded".into())
        );
        assert_eq!(writer.calls, 1, "expired budget performed another write");
    }

    #[test]
    fn endpoint_deadline_rejects_trickling_headers() {
        let server = ScriptedOpenAi::start(BTreeMap::new());
        let mut client = TcpStream::connect(server.address).unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nX-Trickle: ")
            .unwrap();
        await_accept(&server);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let write = client.write_all(b"x");
            if !server.evidence.lock().unwrap().errors.is_empty() {
                break;
            }
            if write.is_err() {
                // The endpoint closes the socket before recording the error;
                // wait for the evidence on the next iteration.
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let errors = server.evidence.lock().unwrap().errors.clone();
        let _ = client.shutdown(Shutdown::Both);
        drop(server);
        assert!(
            errors
                .iter()
                .any(|error| error.contains("connection deadline exceeded")),
            "continuous progress bypassed the total deadline: {errors:?}"
        );
    }
}
