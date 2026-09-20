//! Request observation runs at the HTTP boundary, including retries and routed fallbacks.
//! The observer is task-local so concurrent turns cannot inherit each other's identity.
use crate::{GatewayError, ModelRequest, request_error};
use async_trait::async_trait;
use serde_json::Value;
use std::{future::Future, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Accepted,
    Rejected,
    ConnectionError,
}

#[async_trait]
pub trait RequestObserver: Send + Sync {
    /// Failure prevents dispatch. The returned receipt contains metadata, never credentials.
    async fn starting(
        &self,
        destination: &str,
        request: &ModelRequest,
        body_bytes: usize,
    ) -> Result<Value, GatewayError>;
    async fn finished(&self, receipt: Value, outcome: DispatchOutcome) -> Result<(), GatewayError>;
}

tokio::task_local! {
    static OBSERVER: Arc<dyn RequestObserver>;
}

pub async fn observe<F: Future>(observer: Arc<dyn RequestObserver>, future: F) -> F::Output {
    OBSERVER.scope(observer, future).await
}

pub(crate) async fn send(
    builder: reqwest::RequestBuilder,
    destination: &str,
    request: &ModelRequest,
    body: &Value,
) -> Result<reqwest::Response, GatewayError> {
    // Serialize once; this is the body size, not a file size or a token estimate.
    let bytes = serde_json::to_vec(body)
        .map_err(|error| GatewayError::InvalidResponse(error.to_string()))?;
    let observer = OBSERVER.try_with(Arc::clone).ok();
    let receipt = match &observer {
        Some(observer) => Some(observer.starting(destination, request, bytes.len()).await?),
        None => None,
    };
    let response = builder
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await;
    if let (Some(observer), Some(receipt)) = (observer, receipt) {
        let outcome = match &response {
            Ok(response) if response.status().is_success() => DispatchOutcome::Accepted,
            Ok(_) => DispatchOutcome::Rejected,
            Err(_) => DispatchOutcome::ConnectionError,
        };
        observer.finished(receipt, outcome).await?;
    }
    // Cancellation leaves the durable starting record with an unknown delivery outcome.
    response.map_err(request_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EnvironmentCredentials, ModelMessage, ModelProvider, OpenAiCompatible};
    use axum::{Router, http::StatusCode, routing::post};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Recorder {
        starts: AtomicUsize,
        outcomes: Mutex<Vec<DispatchOutcome>>,
        reject: bool,
    }
    #[async_trait]
    impl RequestObserver for Recorder {
        async fn starting(
            &self,
            _: &str,
            _: &ModelRequest,
            bytes: usize,
        ) -> Result<Value, GatewayError> {
            assert!(bytes > 0);
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.reject {
                return Err(GatewayError::Provider("ledger unavailable".into()));
            }
            Ok(Value::Null)
        }
        async fn finished(&self, _: Value, outcome: DispatchOutcome) -> Result<(), GatewayError> {
            self.outcomes.lock().unwrap().push(outcome);
            Ok(())
        }
    }
    fn request() -> ModelRequest {
        ModelRequest {
            model: "fixture".into(),
            temperature: 0.0,
            messages: vec![ModelMessage {
                role: "user".into(),
                content: Value::String("hello".into()),
            }],
            tools: vec![],
            max_output_tokens: 8,
            routing: None,
        }
    }

    #[tokio::test]
    async fn observes_each_http_dispatch_and_stops_before_network_if_recording_fails() {
        let hits = Arc::new(AtomicUsize::new(0));
        let server_hits = hits.clone();
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let hits = server_hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::TOO_MANY_REQUESTS, "limited")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = OpenAiCompatible::without_auth(endpoint, Arc::new(EnvironmentCredentials));
        let recorder = Arc::new(Recorder::default());
        for _ in 0..2 {
            assert!(
                observe(recorder.clone(), provider.stream(request()))
                    .await
                    .is_err()
            );
        }
        assert_eq!(recorder.starts.load(Ordering::SeqCst), 2);
        assert_eq!(
            *recorder.outcomes.lock().unwrap(),
            vec![DispatchOutcome::Rejected; 2]
        );
        let blocked = Arc::new(Recorder {
            reject: true,
            ..Recorder::default()
        });
        assert!(observe(blocked, provider.stream(request())).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        // A subsequent unscoped call must not reuse the last turn's observer.
        assert!(provider.stream(request()).await.is_err());
        assert_eq!(recorder.starts.load(Ordering::SeqCst), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        server.abort();
    }
    struct FixtureCredentials;
    #[async_trait]
    impl crate::CredentialProvider for FixtureCredentials {
        async fn resolve(&self, _: &str) -> Result<String, GatewayError> {
            Ok("synthetic-key".into())
        }
    }

    #[tokio::test]
    async fn all_builtin_transports_observe_dispatch_and_scopes_do_not_cross() {
        let app = Router::new().fallback(post(|| async { "data: [DONE]\n\n" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let credentials = Arc::new(FixtureCredentials);
        let providers: Vec<Arc<dyn ModelProvider>> = vec![
            Arc::new(OpenAiCompatible::new(
                &origin,
                "fixture",
                credentials.clone(),
            )),
            Arc::new(crate::AnthropicMessages::new(
                &origin,
                "fixture",
                credentials.clone(),
            )),
            Arc::new(crate::GeminiGenerateContent::new(
                &origin,
                "fixture",
                credentials,
            )),
        ];
        let left = Arc::new(Recorder::default());
        let right = Arc::new(Recorder::default());
        for provider in providers {
            let (a, b) = tokio::join!(
                observe(left.clone(), provider.stream(request())),
                observe(right.clone(), provider.stream(request()))
            );
            assert!(a.is_ok() && b.is_ok());
        }
        for recorder in [left, right] {
            assert_eq!(recorder.starts.load(Ordering::SeqCst), 3);
            assert_eq!(
                *recorder.outcomes.lock().unwrap(),
                vec![DispatchOutcome::Accepted; 3]
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_keeps_a_start_without_inventing_a_delivery_outcome() {
        let app = Router::new().fallback(post(|| async {
            std::future::pending::<()>().await;
            "unreachable"
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let provider = OpenAiCompatible::without_auth(origin, Arc::new(FixtureCredentials));
        let recorder = Arc::new(Recorder::default());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                observe(recorder.clone(), provider.stream(request()))
            )
            .await
            .is_err()
        );
        assert_eq!(recorder.starts.load(Ordering::SeqCst), 1);
        assert!(recorder.outcomes.lock().unwrap().is_empty());
        server.abort();
    }
}
