use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tower_http::trace::TraceLayer;

pub const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";
pub const DEFAULT_BIND: &str = "127.0.0.1:18787";
pub const DEFAULT_UPSTREAM: &str = "https://openrouter.ai/api/v1";
const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct ServerConfig {
    pub bind: SocketAddr,
    pub upstream_base_url: String,
    pub openrouter_api_key: String,
    pub client_token: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

impl ServerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("OPENCODING_API_SERVER_BIND")
            .unwrap_or_else(|_| DEFAULT_BIND.into())
            .parse::<SocketAddr>()?;
        let upstream_base_url = std::env::var("OPENCODING_API_SERVER_UPSTREAM")
            .unwrap_or_else(|_| DEFAULT_UPSTREAM.into())
            .trim_end_matches('/')
            .to_owned();
        let openrouter_api_key = match std::env::var("OPENROUTER_API_KEY") {
            Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
            _ => {
                let path = std::env::var("OPENCODING_OPENROUTER_KEY_FILE")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from(".openrouter_apikey"));
                read_key_file(&path)?
            }
        };
        let client_token = std::env::var("OPENCODING_API_SERVER_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let model = std::env::var("OPENCODING_API_SERVER_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.into());
        let reasoning_effort = std::env::var("OPENCODING_API_SERVER_REASONING_EFFORT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim().to_owned());
        if reasoning_effort.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            )
        }) {
            anyhow::bail!(
                "OPENCODING_API_SERVER_REASONING_EFFORT must be one of none, minimal, low, medium, high, xhigh, or max"
            );
        }

        if !bind.ip().is_loopback() && client_token.is_none() {
            anyhow::bail!("OPENCODING_API_SERVER_TOKEN is required when binding outside loopback");
        }

        Ok(Self {
            bind,
            upstream_base_url,
            openrouter_api_key,
            client_token,
            model,
            reasoning_effort,
        })
    }
}

fn read_key_file(path: &Path) -> anyhow::Result<String> {
    let value = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    Ok(value.to_owned())
}

#[derive(Clone)]
struct AppState {
    client: reqwest::Client,
    upstream_base_url: Arc<str>,
    openrouter_api_key: Arc<str>,
    client_token: Option<Arc<str>>,
    model: Arc<str>,
    reasoning_effort: Option<Arc<str>>,
}

pub fn app(config: ServerConfig) -> anyhow::Result<Router> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(90))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("opencoding-api-server/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let state = AppState {
        client,
        upstream_base_url: config.upstream_base_url.into(),
        openrouter_api_key: config.openrouter_api_key.into(),
        client_token: config.client_token.map(Into::into),
        model: config.model.into(),
        reasoning_effort: config.reasoning_effort.map(Into::into),
    };

    Ok(Router::new()
        .route("/healthz", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "model": state.model.as_ref(),
        "reasoning_effort": state.reasoning_effort.as_deref(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(response) = authorization_error(&state, &headers) {
        return response;
    }
    Json(json!({
        "object": "list",
        "data": [{
            "id": state.model.as_ref(),
            "object": "model",
            "owned_by": "opencoding",
        }],
    }))
    .into_response()
}

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    if let Some(response) = authorization_error(&state, &headers) {
        return response;
    }
    let Some(object) = request.as_object_mut() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "request body must be a JSON object",
        );
    };
    if !object.get("messages").is_some_and(Value::is_array) {
        return api_error(StatusCode::BAD_REQUEST, "messages must be an array");
    }
    object.insert("model".into(), Value::String(state.model.to_string()));
    if let Some(effort) = state.reasoning_effort.as_deref() {
        object.insert("reasoning".into(), json!({"effort": effort}));
    }

    let response = match state
        .client
        .post(format!(
            "{}/chat/completions",
            state.upstream_base_url.trim_end_matches('/')
        ))
        .bearer_auth(state.openrouter_api_key.as_ref())
        .header("HTTP-Referer", "https://opencoding.ai")
        .header("X-OpenRouter-Title", "Opencoding")
        .json(&request)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, "OpenRouter request failed");
            return api_error(StatusCode::BAD_GATEWAY, "model provider is unavailable");
        }
    };

    proxy_response(response)
}

fn authorization_error(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let expected = state.client_token.as_deref()?;
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if supplied == Some(expected) {
        return None;
    }
    let mut response = api_error(StatusCode::UNAUTHORIZED, "invalid server token");
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    Some(response)
}

fn proxy_response(upstream: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(header::CACHE_CONTROL).cloned();
    let mut response = Response::new(Body::from_stream(
        upstream
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other)),
    ));
    *response.status_mut() = status;
    if let Some(value) = content_type {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        response.headers_mut().insert(header::CACHE_CONTROL, value);
    }
    response
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "opencoding_api_error"
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn mock_upstream(headers: HeaderMap, Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(
            headers.get(header::AUTHORIZATION).unwrap(),
            "Bearer upstream-secret"
        );
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert!(body.get("reasoning").is_none());
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n",
        )
    }

    async fn mock_reasoning_upstream(Json(body): Json<Value>) -> impl IntoResponse {
        assert_eq!(body["model"], "z-ai/glm-5.3");
        assert_eq!(body["reasoning"], json!({"effort": "low"}));
        (
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: [DONE]\n\n",
        )
    }

    async fn test_app(client_token: Option<&str>) -> Router {
        let upstream = Router::new().route("/chat/completions", post(mock_upstream));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, upstream).into_future());
        app(ServerConfig {
            bind: DEFAULT_BIND.parse().unwrap(),
            upstream_base_url: format!("http://{address}"),
            openrouter_api_key: "upstream-secret".into(),
            client_token: client_token.map(str::to_owned),
            model: DEFAULT_MODEL.into(),
            reasoning_effort: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn forces_default_model_and_streams_provider_response() {
        let response = test_app(None)
            .await
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"model":"ignored","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("hello"));
    }

    #[tokio::test]
    async fn protects_model_routes_when_token_is_configured() {
        let app = test_app(Some("client-secret")).await;
        let rejected = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/models")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let accepted = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/v1/models")
                    .header(header::AUTHORIZATION, "Bearer client-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn forces_configured_model_and_reasoning_effort() {
        let upstream = Router::new().route("/chat/completions", post(mock_reasoning_upstream));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, upstream).into_future());
        let app = app(ServerConfig {
            bind: DEFAULT_BIND.parse().unwrap(),
            upstream_base_url: format!("http://{address}"),
            openrouter_api_key: "upstream-secret".into(),
            client_token: None,
            model: "z-ai/glm-5.3".into(),
            reasoning_effort: Some("low".into()),
        })
        .unwrap();

        let health = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let health = health.into_body().collect().await.unwrap().to_bytes();
        let health: Value = serde_json::from_slice(&health).unwrap();
        assert_eq!(health["model"], "z-ai/glm-5.3");
        assert_eq!(health["reasoning_effort"], "low");

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"model":"ignored","messages":[{"role":"user","content":"hi"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
