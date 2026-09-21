//! A content-free ledger of model HTTP dispatches. Do not infer sends from file reads.
use super::*;
use s_code_model_gateway::{
    GatewayError, ModelStream,
    privacy::{DispatchOutcome, RequestObserver},
};
use s_code_protocol::{PrivacyPage, PrivacyRequest, PrivacySource};

struct Observer {
    state: AppState,
    turn: Turn,
    purpose: &'static str,
}

pub(super) struct ObservedProvider {
    pub inner: Arc<dyn ModelProvider>,
    pub state: AppState,
    pub turn: Turn,
    pub purpose: &'static str,
}

#[async_trait::async_trait]
impl ModelProvider for ObservedProvider {
    async fn stream(&self, mut request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let _dispatch = self
            .state
            .store
            .protection_gate(&self.turn.scope)
            .read_owned()
            .await;
        protection::check_turn(&self.state, &self.turn)
            .await
            .map_err(|_| {
                GatewayError::Provider(
                    "File protection changed. Start a new message with fresh context.".into(),
                )
            })?;
        let policy = self
            .state
            .store
            .file_protection(&self.turn.scope)
            .await
            .map_err(ledger_error)?;
        if !policy.rules.is_empty()
            && request
                .messages
                .iter()
                .any(|message| message.role == "user" && !message.content.is_string())
        {
            return Err(GatewayError::Provider("Hard file protection blocks user attachments and structured content without verified local provenance.".into()));
        }
        if !policy.rules.is_empty() {
            request
                .tools
                .retain(|tool| protection::safe_tool(&tool.name));
        }
        let observer = Arc::new(Observer {
            state: self.state.clone(),
            turn: self.turn.clone(),
            purpose: self.purpose,
        });
        s_code_model_gateway::privacy::observe(observer, self.inner.stream(request)).await
    }
}

fn ledger_error<T>(_: T) -> GatewayError {
    GatewayError::Provider("could not persist the local privacy record".into())
}

impl Observer {
    async fn record(&self, request: &PrivacyRequest, kind: &str) -> Result<(), GatewayError> {
        self.state
            .publish(Event {
                id: Id::new("evt"),
                sequence: 0,
                timestamp: Utc::now(),
                scope: self.turn.scope.clone(),
                session_id: Some(self.turn.session_id.clone()),
                turn_id: Some(self.turn.id.clone()),
                kind: kind.into(),
                payload: serde_json::json!({"item_id": request.id, "request": request}),
            })
            .await
            .map_err(ledger_error)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl RequestObserver for Observer {
    async fn starting(
        &self,
        destination: &str,
        request: &ModelRequest,
        body_bytes: usize,
    ) -> Result<serde_json::Value, GatewayError> {
        let attachments = self
            .state
            .store
            .list_turn_attachments(&self.turn.scope, &self.turn.id)
            .await
            .map_err(ledger_error)?;
        let (sources, unattributed) = sources(request, &attachments);
        let record = PrivacyRequest {
            id: Id::new("disclosure"),
            sequence: 0,
            turn_id: self.turn.id.clone(),
            started_at: Utc::now(),
            destination: destination_origin(destination),
            model: safe_label(&request.model),
            purpose: self.purpose.into(),
            status: "attempted".into(),
            request_bytes: body_bytes as u64,
            sources,
            unattributed,
        };
        self.record(&record, "privacy.request.started").await?;
        serde_json::to_value(record).map_err(ledger_error)
    }

    async fn finished(
        &self,
        receipt: serde_json::Value,
        outcome: DispatchOutcome,
    ) -> Result<(), GatewayError> {
        let mut record: PrivacyRequest = serde_json::from_value(receipt).map_err(ledger_error)?;
        record.status = match outcome {
            DispatchOutcome::Accepted => "accepted",
            DispatchOutcome::Rejected => "rejected",
            DispatchOutcome::ConnectionError => "connection_error",
        }
        .into();
        self.record(&record, "privacy.request.finished").await
    }
}

fn safe_label(value: &str) -> String {
    let mut chars = value.chars();
    let mut label = String::new();
    for character in chars.by_ref().take(4096) {
        if character.is_control() {
            label.extend(character.escape_default());
        } else {
            label.push(character);
        }
    }
    if chars.next().is_some() {
        label.push_str("… [label truncated]");
    }
    label
}

fn destination_origin(value: &str) -> String {
    url::Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "https" | "http"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "Unknown endpoint".into())
}

fn source_label(value: &str) -> String {
    if let Ok(mut url) = url::Url::parse(value) {
        if matches!(url.scheme(), "https" | "http") {
            // A remote source URL may embed credentials in any component, including its path.
            return destination_origin(value);
        }
        url.set_query(None);
        url.set_fragment(None);
        return safe_label(url.as_str());
    }
    safe_label(value)
}

// Never compare a historic read to the current file on disk. This describes
// the captured textual version, not unchanged bytes or binary-file coverage.
fn complete_file_text(result: &serde_json::Value) -> bool {
    if result["truncated"].as_bool() != Some(false) {
        return false;
    }
    let Some(total) = result["total_lines"].as_u64() else {
        return false;
    };
    let Some(numbered) = result["numbered_content"].as_str() else {
        return false;
    };
    let mut count = 0u64;
    for line in numbered.lines() {
        count += 1;
        let Some((prefix, _)) = line.split_once(": ") else {
            return false;
        };
        if prefix.parse::<u64>().ok() != Some(count) {
            return false;
        }
    }
    count == total
}

fn sources(
    request: &ModelRequest,
    attachments: &[Attachment],
) -> (Vec<PrivacySource>, Vec<String>) {
    let mut sources = Vec::new();
    // Older checkpoints and approval resumes may omit the tool name on results.
    // Recover it from the matching structured call, never from user text.
    let calls: BTreeMap<&str, &str> = request
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .filter_map(|message| message.content["tool_calls"].as_array())
        .flatten()
        .filter_map(|call| Some((call["id"].as_str()?, call["function"]["name"].as_str()?)))
        .collect();
    let mut unattributed = BTreeSet::new();
    let mut add = |source: &str, kind: &str, bytes: usize, partial: bool| {
        sources.push(PrivacySource {
            source: source_label(source),
            kind: kind.into(),
            content_bytes: bytes as u64,
            partial,
        });
    };
    for message in &request.messages {
        match message.role.as_str() {
            "system" => {
                if let Some(items) = message.content["context"].as_array() {
                    for item in items {
                        if let (Some(source), Some(content)) =
                            (item["source"].as_str(), item["content"].as_str())
                        {
                            add(
                                source,
                                item["kind"].as_str().unwrap_or("context"),
                                content.len(),
                                true,
                            );
                        }
                    }
                } else {
                    unattributed.insert("System instructions and session metadata".into());
                }
            }
            "tool" => {
                let name = message.content["name"]
                    .as_str()
                    .or_else(|| {
                        message.content["tool_call_id"]
                            .as_str()
                            .and_then(|id| calls.get(id).copied())
                    })
                    .unwrap_or("unknown tool");
                let result = &message.content["result"];
                if name == "read_file"
                    && let (Some(path), Some(content)) = (
                        result["path"].as_str(),
                        result["numbered_content"]
                            .as_str()
                            .or_else(|| result["content"].as_str()),
                    )
                {
                    // Only explicit complete line coverage proves the full captured text.
                    // Older results without that evidence remain conservative excerpts.
                    let complete = complete_file_text(result);
                    add(
                        path,
                        if complete {
                            "file text"
                        } else {
                            "file excerpt"
                        },
                        content.len(),
                        !complete,
                    );
                    continue;
                }
                unattributed.insert(format!(
                    "Tool output: {} (file sources not individually traceable)",
                    safe_label(name)
                ));
            }
            "user" => {
                unattributed.insert("Conversation and pasted text".into());
                if let Some(parts) = message.content.as_array() {
                    let mut matched = BTreeSet::new();
                    for attachment in attachments {
                        let expected = model_content_with_attachments(
                            serde_json::json!(""),
                            std::slice::from_ref(attachment),
                        );
                        let part = &expected[1];
                        if let Some(index) = parts.iter().position(|candidate| candidate == part) {
                            matched.insert(index);
                            let metadata = &attachment.metadata;
                            let metadata_only = part["text"]
                                .as_str()
                                .is_some_and(|text| text.starts_with("\n[Attached file:"));
                            add(
                                &metadata.file_name,
                                if metadata_only {
                                    "attachment name only"
                                } else {
                                    "attachment"
                                },
                                if metadata_only {
                                    0
                                } else {
                                    serde_json::to_vec(part).map_or(0, |bytes| bytes.len())
                                },
                                !metadata.media_type.starts_with("image/")
                                    && metadata.media_type != "application/pdf",
                            );
                        }
                    }
                    if parts
                        .iter()
                        .enumerate()
                        .skip(1)
                        .any(|(index, _)| !matched.contains(&index))
                    {
                        unattributed.insert(
                            "Additional message parts (file sources not individually traceable)"
                                .into(),
                        );
                    }
                }
            }
            _ => {
                unattributed.insert("Conversation, generated text and summaries".into());
            }
        }
    }
    sources.sort_by(|a, b| (&a.source, &a.kind).cmp(&(&b.source, &b.kind)));
    sources.dedup();
    (sources, unattributed.into_iter().collect())
}

#[derive(Deserialize)]
pub(super) struct PrivacyQuery {
    organization_id: String,
    team_id: String,
    actor_id: String,
    before: Option<u64>,
}

pub(super) async fn get_privacy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PrivacyQuery>,
) -> Result<Json<PrivacyPage>, ApiError> {
    let scope = Scope {
        organization_id: Id(query.organization_id),
        team_id: Id(query.team_id),
        actor_id: Id(query.actor_id),
        goal_id: None,
        task_id: None,
    };
    authorize(&state, &headers)?.ensure_scope(&scope)?;
    Ok(Json(
        state
            .store
            .privacy_requests(&scope, &Id(id), query.before, 50)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use s_code_model_gateway::{
        EnvironmentCredentials, GovernedModelRouter, OpenAiCompatible, RoutedModelEndpoint,
    };
    use tower::ServiceExt;

    fn request() -> ModelRequest {
        ModelRequest {
            model: "first".into(),
            temperature: 0.0,
            tools: vec![],
            max_output_tokens: 32,
            routing: None,
            messages: vec![
                ModelMessage {
                    role: "system".into(),
                    content: serde_json::json!({"context": [{"source":"file:///workspace/AGENTS.md","kind":"project_instructions","content":"private-instructions-body"}]}),
                },
                ModelMessage {
                    role: "user".into(),
                    content: serde_json::json!(
                        "Mentioning secret.txt does not prove that file was read"
                    ),
                },
                ModelMessage {
                    role: "tool".into(),
                    content: serde_json::json!({"tool_call_id":"read", "name":"read_file", "result":{"path":"src/main.rs","numbered_content":"1: private-file-body","truncated":false}}),
                },
                ModelMessage {
                    role: "tool".into(),
                    content: serde_json::json!({"tool_call_id":"deny", "name":"read_file", "result":{"error":"access denied"}}),
                },
                ModelMessage {
                    role: "tool".into(),
                    content: serde_json::json!({"tool_call_id":"shell", "name":"run_command", "result":{"stdout":"unattributed-private-output"}}),
                },
            ],
        }
    }

    async fn fixture() -> (AppState, Turn) {
        let store = Store::in_memory().await.unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("alice".into()),
            goal_id: None,
            task_id: None,
        };
        let session = store
            .create_session(CreateSession {
                scope: scope.clone(),
                mode: s_code_protocol::SessionMode::Chat,
                workspace_uri: String::new(),
                title: "Privacy".into(),
                model: "first".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&scope, &session.id).await.unwrap();
        (AppState::new("test-token", store, 0), turn)
    }

    #[test]
    fn privacy_manifest_does_not_infer_files_from_mentions_or_failed_reads() {
        let (files, other) = sources(&request(), &[]);
        assert_eq!(
            files
                .iter()
                .map(|source| source.source.as_str())
                .collect::<Vec<_>>(),
            vec!["file:///workspace/AGENTS.md", "src/main.rs"]
        );
        assert!(files.iter().all(|source| source.partial));
        assert!(other.iter().any(|entry| entry.contains("run_command")));
        let metadata = serde_json::to_string(&files).unwrap();
        assert!(!metadata.contains("private-file-body"));
        assert!(!metadata.contains("private-instructions-body"));
        assert_eq!(safe_label("file\nname.rs"), "file\\nname.rs");
        assert!(safe_label(&"x".repeat(5000)).ends_with("[label truncated]"));
        assert_eq!(
            destination_origin("https://user:password@example.test/v1/token?key=secret#fragment"),
            "https://example.test"
        );
        assert_eq!(
            source_label("https://example.test/secret-token?key=secret"),
            "https://example.test"
        );
    }

    #[test]
    fn privacy_full_text_requires_explicit_complete_line_coverage() {
        let full = serde_json::json!({"path":"src/main.rs", "numbered_content":"1: first\n2: last", "total_lines":2, "truncated":false});
        assert!(complete_file_text(&full));
        for changed in [
            serde_json::json!({"numbered_content":"2: last", "total_lines":2, "truncated":false}),
            serde_json::json!({"numbered_content":"1: first", "total_lines":2, "truncated":false}),
            serde_json::json!({"numbered_content":"1: first\n2: last", "total_lines":2, "truncated":true}),
            serde_json::json!({"numbered_content":"1: first", "truncated":false}),
            serde_json::json!({"content":"first", "total_lines":1, "truncated":false}),
        ] {
            assert!(!complete_file_text(&changed));
        }
        let mut outgoing = request();
        outgoing.messages[2].content["result"] = full;
        let manifest = sources(&outgoing, &[]).0;
        let file = manifest
            .iter()
            .find(|source| source.source == "src/main.rs")
            .unwrap();
        assert!(!file.partial);
        assert_eq!(file.kind, "file text");
    }

    #[test]
    fn privacy_recovers_read_provenance_after_approval_resume() {
        let mut outgoing = request();
        outgoing.messages[2]
            .content
            .as_object_mut()
            .unwrap()
            .remove("name");
        outgoing.messages.insert(2, ModelMessage { role: "assistant".into(), content: serde_json::json!({"tool_calls":[{"id":"read","function":{"name":"read_file","arguments":"{}"}}]}) });
        assert!(
            sources(&outgoing, &[])
                .0
                .iter()
                .any(|source| source.source == "src/main.rs")
        );
    }

    #[test]
    fn privacy_attachment_is_reported_only_when_its_payload_is_present() {
        let attachment = Attachment {
            metadata: AttachmentMetadata {
                id: Id("attachment".into()),
                session_id: Id("session".into()),
                turn_id: None,
                file_name: "design.png".into(),
                media_type: "image/png".into(),
                byte_length: 4,
                sha256: "hash".into(),
                created_at: Utc::now(),
            },
            content_base64: STANDARD.encode(b"fake"),
        };
        let mut request = request();
        let content = model_content_with_attachments(
            serde_json::json!("Look"),
            std::slice::from_ref(&attachment),
        );
        request.messages.push(ModelMessage {
            role: "user".into(),
            content,
        });
        assert!(
            sources(&request, std::slice::from_ref(&attachment))
                .0
                .iter()
                .any(|source| source.source == "design.png")
        );
        request.messages.pop(); // Simulate removal during compaction: no phantom disclosure.
        assert!(
            !sources(&request, &[attachment])
                .0
                .iter()
                .any(|source| source.source == "design.png")
        );
    }

    #[tokio::test]
    async fn privacy_records_fallback_endpoints_and_enforces_account_scope_and_pagination() {
        let server_app = Router::new()
            .route(
                "/limited/chat/completions",
                post(|| async { StatusCode::TOO_MANY_REQUESTS }),
            )
            .route(
                "/ok/chat/completions",
                post(|| async { "data: [DONE]\n\n" }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, server_app).await.unwrap();
        });
        let (state, turn) = fixture().await;
        let endpoints = [
            ("first", "limited", "upstream-one"),
            ("second", "ok", "upstream-two"),
        ]
        .map(|(id, path, model)| RoutedModelEndpoint {
            id: id.into(),
            provider_model: model.into(),
            provider: Arc::new(OpenAiCompatible::without_auth(
                format!("{origin}/{path}"),
                Arc::new(EnvironmentCredentials),
            )),
        });
        let provider = ObservedProvider {
            inner: Arc::new(GovernedModelRouter::new(endpoints.to_vec()).unwrap()),
            state: state.clone(),
            turn: turn.clone(),
            purpose: "agent",
        };
        let mut outgoing = request();
        outgoing.routing = Some(ModelRoutingPolicy {
            routing_order: vec!["first".into(), "second".into()],
            fallback_reasons: vec![FallbackReason::RateLimited],
        });
        let _stream = provider.stream(outgoing).await.unwrap();
        let page = state
            .store
            .privacy_requests(&turn.scope, &turn.session_id, None, 1)
            .await
            .unwrap();
        assert_eq!(page.requests.len(), 1);
        assert_eq!(page.requests[0].status, "accepted");
        assert_eq!(page.requests[0].model, "upstream-two");
        assert_eq!(page.requests[0].destination, origin);
        assert!(page.requests[0].request_bytes > 0);
        let older = state
            .store
            .privacy_requests(&turn.scope, &turn.session_id, page.next_before, 1)
            .await
            .unwrap();
        assert_eq!(older.requests[0].status, "rejected");
        assert_eq!(older.requests[0].model, "upstream-one");
        assert!(older.next_before.is_none());
        let encoded = serde_json::to_string(&page).unwrap();
        for secret in [
            "private-file-body",
            "private-instructions-body",
            "unattributed-private-output",
            "test-token",
        ] {
            assert!(!encoded.contains(secret));
        }
        let service = app(state.clone());
        for (actor, token, expected) in [
            ("alice", "test-token", StatusCode::OK),
            ("bob", "test-token", StatusCode::FORBIDDEN),
            ("alice", "wrong-token", StatusCode::UNAUTHORIZED),
        ] {
            let response = service.clone().oneshot(Request::builder()
                .uri(format!("/v1/sessions/{}/privacy?organization_id=org&team_id=team&actor_id={actor}", turn.session_id.0))
                .header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::OK {
                let page: PrivacyPage =
                    serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                        .unwrap();
                assert_eq!(page.requests.len(), 2);
            }
        }
        // A cancelled dispatch retains the starting event instead of disappearing or claiming success.
        let observer = Observer {
            state: state.clone(),
            turn: turn.clone(),
            purpose: "session_title",
        };
        observer.starting(&origin, &request(), 42).await.unwrap();
        let page = state
            .store
            .privacy_requests(&turn.scope, &turn.session_id, None, 1)
            .await
            .unwrap();
        assert_eq!(page.requests[0].status, "attempted");
        assert_eq!(page.requests[0].purpose, "session_title");
        server.abort();
    }
}
