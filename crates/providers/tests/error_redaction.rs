use httpmock::prelude::*;
use locust_core::models::TranslationRequest;
use locust_core::translation::TranslationProvider;
use locust_providers::{
    claude::ClaudeProvider, deepl::DeepLProvider, ollama::OllamaProvider, openai::OpenAiProvider,
};

#[tokio::test]
async fn provider_http_errors_redact_secrets_before_truncating() {
    let server = MockServer::start();
    let sentinel = "configured-credential-sentinel";
    let other_token = "other-bearer-sentinel";
    let body = format!(
        "Rejected {sentinel}; Authorization: Bearer {other_token}\nx-api-key: header-secret\nhttps://example.test?key=query-secret&ok=yes sk-shape-secret {}{sentinel}",
        "日".repeat(700)
    );
    let mock = server.mock(|when, then| {
        when.method(POST).body_contains("Hello");
        then.status(401).body(&body);
    });
    let mut providers: Vec<Box<dyn TranslationProvider>> = vec![
        Box::new(OpenAiProvider::new(
            sentinel.into(),
            None,
            Some(server.base_url()),
        )),
        Box::new(ClaudeProvider::new(
            sentinel.into(),
            None,
            Some(server.base_url()),
        )),
        Box::new(DeepLProvider::with_base_url(
            sentinel.into(),
            true,
            server.base_url(),
        )),
        Box::new(OllamaProvider::new(Some(server.base_url()), None)),
    ];
    for id in ["deepseek", "gemini", "grok", "grok-sub"] {
        providers.push(Box::new(OpenAiProvider::compatible(
            id.into(),
            id.into(),
            sentinel.into(),
            server.base_url(),
            "test-model".into(),
        )));
    }
    let request = TranslationRequest {
        entry_id: "test".into(),
        source: "Hello".into(),
        source_lang: "en".into(),
        target_lang: "es".into(),
        context: None,
        glossary_hint: None,
    };
    for provider in providers {
        let error = provider
            .translate(std::slice::from_ref(&request))
            .await
            .unwrap_err()
            .to_string();
        // Ollama has no configured credential, but must still remove common shapes.
        if provider.id() != "ollama" {
            assert!(
                !error.contains(sentinel),
                "{} leaked configured credential",
                provider.id()
            );
        }
        for secret in [
            other_token,
            "header-secret",
            "query-secret",
            "sk-shape-secret",
        ] {
            assert!(!error.contains(secret), "{} leaked {secret}", provider.id());
        }
        assert!(error.contains("401"));
        assert!(error.contains("[REDACTED]"));
        assert!(error.ends_with('…'));
        assert!(
            error.chars().count() <= 560,
            "unbounded error: {}",
            provider.id()
        );
    }
    assert_eq!(mock.hits(), 8);
}

#[tokio::test]
async fn xai_device_errors_redact_common_secret_shapes() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST);
        then.status(400)
            .body("Authorization: Bearer oauth-sentinel\nx-api-key: header-sentinel");
    });
    let error = locust_providers::xai_oauth::request_device_code_at(&server.base_url())
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("oauth-sentinel"));
    assert!(!error.contains("header-sentinel"));
    assert!(error.contains("400"));
}

#[tokio::test]
async fn malformed_success_response_cannot_echo_credentials_in_parse_errors() {
    let server = MockServer::start();
    let secret = "configured-parse-sentinel";
    let echoed = format!("{secret} Bearer parse-bearer-sentinel {}", "x".repeat(700));
    server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200)
            .json_body(serde_json::json!({"choices":[{"message":{"content": echoed}}]}));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v1/messages");
        then.status(200)
            .json_body(serde_json::json!({"content":[{"text": echoed}]}));
    });
    server.mock(|when, then| {
        when.method(POST).path("/api/chat");
        then.status(200)
            .json_body(serde_json::json!({"message":{"content": echoed}}));
    });
    let providers: Vec<Box<dyn TranslationProvider>> = vec![
        Box::new(OpenAiProvider::new(
            secret.into(),
            None,
            Some(server.base_url()),
        )),
        Box::new(ClaudeProvider::new(
            secret.into(),
            None,
            Some(server.base_url()),
        )),
        Box::new(OllamaProvider::new(Some(server.base_url()), None)),
    ];
    let request = TranslationRequest {
        entry_id: "test".into(),
        source: "Hello".into(),
        source_lang: "en".into(),
        target_lang: "es".into(),
        context: None,
        glossary_hint: None,
    };
    for provider in providers {
        let error = provider
            .translate(std::slice::from_ref(&request))
            .await
            .unwrap_err()
            .to_string();
        if provider.id() != "ollama" {
            assert!(!error.contains(secret));
        }
        assert!(!error.contains("parse-bearer-sentinel"));
        assert!(error.chars().count() <= 560);
    }
}

#[tokio::test]
async fn transport_errors_redact_credential_query_parameters() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/?key=query-transport-sentinel",
        listener.local_addr().unwrap()
    );
    drop(listener);
    let providers: Vec<Box<dyn TranslationProvider>> = vec![
        Box::new(OpenAiProvider::new("key".into(), None, Some(url.clone()))),
        Box::new(ClaudeProvider::new("key".into(), None, Some(url.clone()))),
        Box::new(DeepLProvider::with_base_url(
            "key".into(),
            true,
            url.clone(),
        )),
        Box::new(OllamaProvider::new(Some(url), None)),
    ];
    for provider in providers {
        let error = provider.health_check().await.unwrap_err().to_string();
        assert!(
            !error.contains("query-transport-sentinel"),
            "{} leaked URL credentials",
            provider.id()
        );
    }
}

#[tokio::test]
async fn xai_login_errors_redact_device_codes() {
    use locust_providers::xai_oauth::{poll_for_token_at, DeviceCode};
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST)
            .body_contains("device_code=device-code-sentinel");
        then.status(400).json_body(
            serde_json::json!({"error":"device-code-sentinel Bearer oauth-error-sentinel"}),
        );
    });
    let device = DeviceCode::from_parts(
        "device-code-sentinel",
        "ABCD",
        "https://example.test",
        1,
        900,
        u64::MAX,
    );
    let error = poll_for_token_at(&server.base_url(), &device)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("device-code-sentinel"));
    assert!(!error.contains("oauth-error-sentinel"));
}

// Capture actual tracing fields without adding a test-only logging dependency.
#[derive(Clone, Default)]
struct ErrorLogs(std::sync::Arc<std::sync::Mutex<String>>);

impl tracing::Subscriber for ErrorLogs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl tracing::field::Visit for Fields<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                writeln!(self.0, "{value:?}").unwrap();
            }
        }
        event.record(&mut Fields(&mut self.0.lock().unwrap()));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

async fn assert_error_event_and_logs_redacted(
    provider: std::sync::Arc<dyn TranslationProvider>,
    sentinels: &[&str],
) {
    use locust_core::translation::{RetryConfig, TranslationManager, TranslationOptions};
    use locust_core::{
        database::Database,
        glossary::Glossary,
        models::{ProgressEvent, StringEntry},
    };
    use std::sync::Arc;

    let request = TranslationRequest {
        entry_id: "redaction".into(),
        source: "Hello".into(),
        source_lang: "en".into(),
        target_lang: "es".into(),
        context: None,
        glossary_hint: None,
    };
    let error = provider
        .translate(&[request])
        .await
        .unwrap_err()
        .to_string();
    let logs = ErrorLogs::default();
    let _subscriber = tracing::subscriber::set_default(logs.clone());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())))
            .with_retry_config(RetryConfig {
                max_attempts: 1,
                ..Default::default()
            });
    let entry = StringEntry::new("redaction", "Hello", "story.txt".into());
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    manager
        .translate_entries(
            vec![entry],
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "redaction-job".into(),
            Default::default(),
        )
        .await
        .unwrap();
    let mut failures = Vec::new();
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::BatchFailed { error, .. } = event {
            failures.push(error);
        }
    }
    assert!(!failures.is_empty(), "expected a real BatchFailed event");
    let logged = logs.0.lock().unwrap().clone();
    assert!(
        logged.contains("Batch translation failed"),
        "expected an actual manager log: {logged}"
    );
    for (surface, value) in [
        ("returned error", error),
        ("BatchFailed", failures.join("\n")),
        ("log output", logged),
    ] {
        for sentinel in sentinels {
            assert!(
                !value.contains(sentinel),
                "{surface} leaked {sentinel}: {value}"
            );
        }
    }
}

#[tokio::test]
async fn json_header_strings_and_arrays_redact_errors_events_and_logs() {
    for header in [
        "X-Api-Key",
        "Authorization",
        "aPi-KeY",
        "X-GOOG-API-KEY",
        "cOoKiE",
    ] {
        for value in [
            r#"["array-header-sentinel", "second-array-sentinel"]"#,
            r#""string-header-sentinel""#,
        ] {
            let server = MockServer::start();
            let body = format!(
                r#"prefix {{"{header}":{value},"safe":"diagnostic"}} configured-exact-sentinel"#
            );
            let mock = server.mock(|when, then| {
                when.method(POST);
                then.status(401).body(body);
            });
            let provider = std::sync::Arc::new(OpenAiProvider::new(
                "configured-exact-sentinel".into(),
                None,
                Some(server.base_url()),
            ));
            assert_error_event_and_logs_redacted(
                provider,
                &[
                    "string-header-sentinel",
                    "array-header-sentinel",
                    "second-array-sentinel",
                    "configured-exact-sentinel",
                ],
            )
            .await;
            assert_eq!(mock.hits(), 2);
        }
    }
}

#[tokio::test]
async fn json_escaped_bearer_whitespace_redacts_errors_events_and_logs() {
    for separator in [r"\t", r"\n", r"\r", r"\f", r"\u0020", r"\u0009"] {
        let server = MockServer::start();
        let body = format!(r#"{{"Authorization":"Bearer{separator}escaped-bearer-sentinel"}}"#);
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(401).body(body);
        });
        let provider = std::sync::Arc::new(OpenAiProvider::new(
            "configured-key".into(),
            None,
            Some(server.base_url()),
        ));
        assert_error_event_and_logs_redacted(provider, &["escaped-bearer-sentinel"]).await;
        assert_eq!(mock.hits(), 2);
    }
}

// Exercise the real OAuth request functions through the translation manager too.
struct OAuthErrorProbe {
    url: String,
    poll: bool,
}

#[async_trait::async_trait]
impl TranslationProvider for OAuthErrorProbe {
    fn id(&self) -> &str {
        "oauth-redaction-probe"
    }
    fn name(&self) -> &str {
        "OAuth redaction probe"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(
        &self,
        _: &[TranslationRequest],
    ) -> locust_core::error::Result<Vec<locust_core::models::TranslationResult>> {
        use locust_providers::xai_oauth::{poll_for_token_at, request_device_code_at, DeviceCode};
        if self.poll {
            let device = DeviceCode::from_parts(
                "device-exact-sentinel",
                "ABCD",
                "https://example.test",
                1,
                900,
                u64::MAX,
            );
            poll_for_token_at(&self.url, &device).await?;
        } else {
            request_device_code_at(&self.url).await?;
        }
        panic!("expected an OAuth error")
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }
    async fn health_check(&self) -> locust_core::error::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn oauth_transport_queries_redact_errors_events_and_logs() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    for parameter in ["key", "api_key", "access_token", "token"] {
        for poll in [false, true] {
            let provider = std::sync::Arc::new(OAuthErrorProbe {
                url: format!("http://{address}/?{parameter}=oauth-query-sentinel&safe=yes"),
                poll,
            });
            assert_error_event_and_logs_redacted(provider, &["oauth-query-sentinel"]).await;
        }
    }
}

#[tokio::test]
async fn malformed_provider_responses_redact_url_queries() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST);
        then.status(200).body("not JSON");
    });
    let url = format!("{}/?token=decode-query-sentinel", server.base_url());
    let providers: Vec<std::sync::Arc<dyn TranslationProvider>> = vec![
        std::sync::Arc::new(OpenAiProvider::new(
            "configured-key".into(),
            None,
            Some(url.clone()),
        )),
        std::sync::Arc::new(ClaudeProvider::new(
            "configured-key".into(),
            None,
            Some(url.clone()),
        )),
        std::sync::Arc::new(DeepLProvider::with_base_url(
            "configured-key".into(),
            true,
            url.clone(),
        )),
        std::sync::Arc::new(OllamaProvider::new(Some(url.clone()), None)),
        std::sync::Arc::new(locust_providers::argos::ArgosProvider::new(url.clone())),
        std::sync::Arc::new(OAuthErrorProbe {
            url: url.clone(),
            poll: false,
        }),
        std::sync::Arc::new(OAuthErrorProbe { url, poll: true }),
    ];
    for provider in providers {
        assert_error_event_and_logs_redacted(provider, &["decode-query-sentinel"]).await;
    }
}

#[tokio::test]
async fn argos_transport_errors_redact_url_queries() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/?token=argos-query-sentinel",
        listener.local_addr().unwrap()
    );
    drop(listener);
    let provider = std::sync::Arc::new(locust_providers::argos::ArgosProvider::new(url));
    assert_error_event_and_logs_redacted(provider.clone(), &["argos-query-sentinel"]).await;
    let error = provider.health_check().await.unwrap_err().to_string();
    assert!(!error.contains("argos-query-sentinel"), "{error}");
}

// New sentinels are ASCII and deliberately distinct from log metadata.
fn sentinel_fragments(sentinel: &str) -> Vec<&str> {
    assert!(sentinel.is_ascii() && sentinel.len() >= 6);
    (0..=sentinel.len() - 6)
        .map(|start| &sentinel[start..start + 6])
        .collect()
}

#[tokio::test]
async fn conservative_credential_boundaries_redact_errors_events_and_logs() {
    // Each row goes through a real HTTP error, manager event, and tracing log.
    let cases = [
        (
            "comma query",
            r"https://example.test?token=prefix,commaquerysentinel&safe=yes",
            "commaquerysentinel",
        ),
        (
            "slash query",
            r"https://example.test?key=prefix/slashquerysentinel&safe=yes",
            "slashquerysentinel",
        ),
        (
            "escaped slash bearer",
            r#"{"message":"Bearer prefix\/slashbearersentinel"}"#,
            "slashbearersentinel",
        ),
        (
            "backslash query",
            r"https://example.test?api_key=prefix\backslashquerysentinel&safe=yes",
            "backslashquerysentinel",
        ),
        (
            "percent query",
            r"https://example.test?access_token=prefix%2Fpercentquerysentinel&safe=yes",
            "percentquerysentinel",
        ),
        (
            "plus query",
            r"https://example.test?client_secret=prefix+plusquerysentinel&safe=yes",
            "plusquerysentinel",
        ),
        (
            "dot query",
            r"https://example.test?refresh_token=prefix.dotquerysentinel&safe=yes",
            "dotquerysentinel",
        ),
        (
            "equals query",
            r"https://example.test?token=prefix=equalsquerysentinel&safe=yes",
            "equalsquerysentinel",
        ),
        (
            "trailing quote",
            r#"{"message":"Bearer prefix,trailingquotesentinel","safe":"diagnostic"}"#,
            "trailingquotesentinel",
        ),
        (
            "query fragment",
            r"https://example.test?token=prefix,fragmentsentinel#safe",
            "fragmentsentinel",
        ),
        (
            "unicode escape bearer",
            r#"{"message":"Bearer prefix\u002f.unicodeescapesentinel"}"#,
            "unicodeescapesentinel",
        ),
        (
            "escaped quote bearer",
            r#"{"message":"Bearer prefix\".escapedquotesentinel"}"#,
            "escapedquotesentinel",
        ),
        (
            "escaped backslash bearer",
            r#"{"message":"Bearer prefix\\.escapedbackslashsentinel"}"#,
            "escapedbackslashsentinel",
        ),
        (
            "escaped whitespace bearer",
            r#"{"message":"Bearer prefix\t.escapedwhitespacesentinel"}"#,
            "escapedwhitespacesentinel",
        ),
        (
            "basic scheme",
            r#"{"message":"Basic prefix\/basicschemesentinel"}"#,
            "basicschemesentinel",
        ),
        (
            "token scheme",
            r#"{"message":"Token prefix\/tokenschemesentinel"}"#,
            "tokenschemesentinel",
        ),
    ];
    for (name, body, sentinel) in cases {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(401).body(body);
        });
        let provider = std::sync::Arc::new(OpenAiProvider::new(
            "configured-key".into(),
            None,
            Some(server.base_url()),
        ));
        assert_error_event_and_logs_redacted(provider, &sentinel_fragments(sentinel)).await;
        assert_eq!(mock.hits(), 2, "{name}");
    }
}

#[tokio::test]
async fn encoded_configured_secrets_redact_errors_events_and_logs() {
    let secret = "opaque/credential+qzvkmnrp";
    for body in [
        secret.to_owned(),
        r"rejected opaque\/credential+qzvkmnrp".to_owned(),
        r"rejected opaque\u002fcredential+qzvkmnrp".to_owned(),
        r#"{"message":"opaque\/credential+qzvkmnrp"}"#.to_owned(),
        r#"{"message":"opaque\u002fcredential+qzvkmnrp"}"#.to_owned(),
        "opaque%2Fcredential%2Bqzvkmnrp".to_owned(),
        "opaque%2fcredential%2bqzvkmnrp".to_owned(),
        "%6f%70%61%71%75%65%2F%63%72%65%64%65%6E%74%69%61%6C%2Bqzvkmnrp".to_owned(),
    ] {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST);
            then.status(401).body(body);
        });
        let provider = std::sync::Arc::new(OpenAiProvider::new(
            secret.into(),
            None,
            Some(server.base_url()),
        ));
        assert_error_event_and_logs_redacted(provider, &sentinel_fragments(secret)).await;
        assert_eq!(mock.hits(), 2);
    }
}
