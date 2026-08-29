use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, HeaderValue, Response, StatusCode, header::CONTENT_TYPE},
    response::IntoResponse,
    routing::post,
};
use opencoding_mcp_client::{
    McpElicitationHandler, McpElicitationRequest, McpElicitationResponse, McpError,
    McpHttpServerConfig, McpRegistry,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::{Mutex, Notify, mpsc};

#[derive(Clone, Default)]
struct FixtureState {
    initialized: Arc<AtomicUsize>,
    elicitation_reply: Arc<Mutex<Option<Value>>>,
    elicitation_notify: Arc<Notify>,
}

async fn mcp_endpoint(
    State(state): State<FixtureState>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response<Body> {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if method != "initialize"
        && (headers
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            != Some("fixture-session")
            || headers
                .get("mcp-protocol-version")
                .and_then(|value| value.to_str().ok())
                != Some("2025-06-18"))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if method.is_empty() && message["id"] == "http-elicitation" {
        *state.elicitation_reply.lock().await = message.get("result").cloned();
        // Preserve one permit if the SSE producer has not polled `notified`
        // yet; `notify_waiters` would lose that edge and deadlock the fixture.
        state.elicitation_notify.notify_one();
        return StatusCode::ACCEPTED.into_response();
    }
    match method {
        "initialize" => {
            state.initialized.fetch_add(1, Ordering::Relaxed);
            let mut response = Json(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}, "resources": {}},
                    "serverInfo": {"name": "fixture", "version": "1"}
                }
            }))
            .into_response();
            response.headers_mut().insert(
                "mcp-session-id",
                HeaderValue::from_static("fixture-session"),
            );
            response
        }
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => Json(json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "Echo one value",
                    "inputSchema": {"type": "object"},
                    "outputSchema": {"type": "object"}
                }, {
                    "name": "elicit",
                    "description": "Request reviewed input",
                    "inputSchema": {"type": "object"},
                    "outputSchema": {"type": "object"}
                }]
            }
        }))
        .into_response(),
        "tools/call" => {
            let id = message["id"].as_u64().unwrap();
            if message["params"]["name"] == "elicit" {
                let (sender, receiver) =
                    mpsc::channel::<Result<Bytes, std::convert::Infallible>>(2);
                let fixture = state.clone();
                tokio::spawn(async move {
                    let request = format!(
                        "event: message\ndata: {}\n\n",
                        json!({
                            "jsonrpc": "2.0",
                            "id": "http-elicitation",
                            "method": "elicitation/create",
                            "params": {
                                "message": "Choose a reviewed display name.",
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {"display_name": {"type": "string"}},
                                    "required": ["display_name"]
                                }
                            }
                        })
                    );
                    sender.send(Ok(Bytes::from(request))).await.unwrap();
                    fixture.elicitation_notify.notified().await;
                    let content = fixture.elicitation_reply.lock().await.clone().unwrap();
                    let response = format!(
                        "event: message\ndata: {}\n\n",
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{
                                    "type": "text",
                                    "text": content["content"]["display_name"]
                                }],
                                "isError": false
                            }
                        })
                    );
                    sender.send(Ok(Bytes::from(response))).await.unwrap();
                });
                let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
                    receiver.recv().await.map(|item| (item, receiver))
                });
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap();
            }
            let progress_token = message["params"]["_meta"]["progressToken"]
                .as_str()
                .unwrap();
            let body = format!(
                "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {
                        "progressToken": progress_token,
                        "progress": 1,
                        "total": 1,
                        "message": "done"
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": "http result"}],
                        "isError": false
                    }
                })
            );
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }
        "resources/list" => Json(json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "resources": [{
                    "uri": "fixture://readme",
                    "name": "README",
                    "description": "Fixture documentation",
                    "mimeType": "text/markdown"
                }]
            }
        }))
        .into_response(),
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "error": {"code": -32601, "message": "not found"}
        }))
        .into_response(),
    }
}

struct HttpElicitation;

#[async_trait::async_trait]
impl McpElicitationHandler for HttpElicitation {
    async fn elicit(
        &self,
        request: McpElicitationRequest,
    ) -> Result<McpElicitationResponse, McpError> {
        assert_eq!(request.request_id, "http-elicitation");
        Ok(McpElicitationResponse::Accept {
            content: json!({"display_name": "HTTP reviewed"}),
        })
    }
}

#[tokio::test]
async fn streamable_http_negotiates_session_and_projects_tools_resources_and_progress() {
    let fixture = FixtureState::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(mcp_endpoint))
                .with_state(fixture.clone()),
        )
        .into_future(),
    );
    let registry = McpRegistry::connect_with_http(
        &[],
        &[McpHttpServerConfig {
            id: "remote".into(),
            endpoint,
            header_handles: BTreeMap::new(),
            timeout_ms: 5_000,
        }],
    )
    .await
    .unwrap();
    assert_eq!(fixture.initialized.load(Ordering::Relaxed), 1);
    assert_eq!(registry.definitions()[0].name, "mcp.remote.echo");
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(4);
    let result = registry
        .call_with_progress(
            "mcp.remote.echo",
            json!({"value": "safe"}),
            Some(progress_tx),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "http result");
    let progress = progress_rx.recv().await.unwrap();
    assert_eq!(progress.progress, 1.0);
    assert_eq!(progress.message.as_deref(), Some("done"));
    let resources = registry.list_resources("remote", None).await.unwrap();
    assert_eq!(resources.entries[0].uri, "fixture://readme");
    let elicited = registry
        .call_with_progress_and_elicitation(
            "mcp.remote.elicit",
            json!({}),
            None,
            Some(Arc::new(HttpElicitation)),
        )
        .await
        .unwrap();
    assert_eq!(elicited["content"][0]["text"], "HTTP reviewed");
    server.abort();
}

#[test]
fn streamable_http_rejects_remote_cleartext_credentials_and_url_smuggling() {
    for endpoint in [
        "http://example.com/mcp",
        "https://user@example.com/mcp",
        "https://example.com/mcp?token=secret",
        "https://example.com/mcp#fragment",
    ] {
        let configuration = McpHttpServerConfig {
            id: "remote".into(),
            endpoint: endpoint.into(),
            header_handles: BTreeMap::new(),
            timeout_ms: 5_000,
        };
        assert!(
            configuration.validate().is_err(),
            "unsafe endpoint unexpectedly accepted: {endpoint}"
        );
    }
}
