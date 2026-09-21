//! Regression tests for the account-wide hard file-protection boundary.
use super::*;
use axum::body::to_bytes;
use s_code_model_gateway::{GatewayError, ModelStream};
use s_code_storage::FileProtectionPolicy;
use tower::ServiceExt;

struct Fixture {
    state: AppState,
    session: Session,
    directory: tempfile::TempDir,
    database: String,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("private.txt"), "FILE_SECRET_SENTINEL").unwrap();
        let database = format!(
            "sqlite://{}",
            directory.path().join("state/test.sqlite").display()
        );
        let store = Store::connect(&database).await.unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("alice".into()),
            goal_id: None,
            task_id: None,
        };
        let session = store
            .create_session(CreateSession {
                scope,
                mode: s_code_protocol::SessionMode::Work,
                workspace_uri: url::Url::from_directory_path(&workspace)
                    .unwrap()
                    .to_string(),
                title: "New conversation".into(),
                model: "fixture".into(),
            })
            .await
            .unwrap();
        Self {
            state: AppState::new("test-token", store, 0),
            session,
            directory,
            database,
        }
    }
    fn protections(&self) -> String {
        format!("/v1/sessions/{}/privacy/protections", self.session.id.0)
    }
    async fn protect(&self) -> FileProtectionPolicy {
        let (status, body) = http(&self.state, "POST", &self.protections(), Some(serde_json::json!({"scope": self.session.scope, "path":"private.txt", "expected_revision":0})), Some("test-token")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }
}

async fn http(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app(state.clone())
        .oneshot(
            request
                .body(Body::from(
                    body.map(|body| body.to_string()).unwrap_or_default(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::json!(String::from_utf8_lossy(&bytes)));
    (status, body)
}

async fn user_turn(f: &Fixture, text: &str) -> Turn {
    f.state
        .store
        .create_turn_with_user_message_and_attachments_if_idle(
            &f.session.scope,
            &f.session.id,
            serde_json::json!(text),
            &[],
        )
        .await
        .unwrap()
        .0
}
async fn complete(f: &Fixture, turn: &Turn) {
    for status in [
        TurnStatus::PreparingContext,
        TurnStatus::CallingModel,
        TurnStatus::Completed,
    ] {
        f.state
            .store
            .update_turn(&turn.scope, &turn.id, status, None, None)
            .await
            .unwrap();
    }
}

#[derive(Default)]
struct CaptureProvider {
    requests: StdMutex<Vec<ModelRequest>>,
    entered: tokio::sync::Notify,
    release: Option<Arc<tokio::sync::Semaphore>>,
}
#[async_trait::async_trait]
impl ModelProvider for CaptureProvider {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        self.requests.lock().unwrap().push(request);
        self.entered.notify_one();
        if let Some(release) = &self.release {
            release.acquire().await.unwrap().forget();
        }
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                text: "Safe fresh conversation".into(),
            }),
            Ok(ModelEvent::Completed {
                finish_reason: Some("stop".into()),
            }),
        ])))
    }
}

#[tokio::test]
async fn protection_api_auth_owner_cas_and_persisted_empty_cutoff() {
    let f = Fixture::new().await;
    let query = "organization_id=org&team_id=team&actor_id=alice";
    let url = format!("{}?{query}", f.protections());
    for token in [None, Some("incorrect")] {
        assert_eq!(
            http(&f.state, "GET", &url, None, token).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        http(
            &f.state,
            "GET",
            &format!(
                "{}?organization_id=org&team_id=team&actor_id=bob",
                f.protections()
            ),
            None,
            Some("test-token")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let foreign = Scope {
        actor_id: Id("bob".into()),
        ..f.session.scope.clone()
    };
    assert_eq!(
        http(
            &f.state,
            "POST",
            &f.protections(),
            Some(serde_json::json!({"scope":foreign,"path":"private.txt","expected_revision":0})),
            Some("test-token")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let first = f.protect().await;
    assert_eq!(first.revision, 1);
    assert_eq!(first.rules.len(), 1);
    assert!(first.changed_at.is_some());
    assert_eq!(http(&f.state, "POST", &f.protections(), Some(serde_json::json!({"scope":f.session.scope,"path":"another.txt","expected_revision":0})), Some("test-token")).await.0, StatusCode::CONFLICT);
    let delete = format!("{}/{}", f.protections(), first.rules[0].id.0);
    assert_eq!(
        http(
            &f.state,
            "DELETE",
            &format!("{delete}?organization_id=org&team_id=team&actor_id=bob&expected_revision=1"),
            None,
            Some("test-token")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http(
            &f.state,
            "DELETE",
            &format!("{delete}?{query}&expected_revision=0"),
            None,
            Some("test-token")
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, body) = http(
        &f.state,
        "DELETE",
        &format!("{delete}?{query}&expected_revision=1"),
        None,
        Some("test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let empty: FileProtectionPolicy = serde_json::from_value(body).unwrap();
    assert!(empty.rules.is_empty());
    assert_eq!(empty.revision, 2);
    assert!(empty.changed_at > first.changed_at);
    let reopened = Store::connect(&f.database).await.unwrap();
    assert_eq!(
        reopened.file_protection(&f.session.scope).await.unwrap(),
        empty
    );
    let events = f
        .state
        .store
        .list_events(&f.session.scope.team_id, 0, 100)
        .await
        .unwrap();
    let updates = events
        .iter()
        .filter(|event| event.kind == "privacy.protection.updated")
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 2);
    assert!(
        updates
            .iter()
            .all(|event| event.payload["context_reset"] == true)
    );
    assert!(
        !serde_json::to_string(&updates)
            .unwrap()
            .contains("private.txt")
    );
}

#[tokio::test]
async fn protection_chat_command_completes_locally_without_configured_provider() {
    let f = Fixture::new().await;
    assert!(f.state.model_provider.is_none());
    let turn = start_agent_turn(
        f.state.clone(),
        f.session.id.clone(),
        f.session.scope.clone(),
        serde_json::json!("/protect private.txt"),
        vec![],
        ToolProfile::Default,
        true,
    )
    .await
    .unwrap();
    assert_eq!(turn.status, TurnStatus::Completed);
    assert_eq!(
        f.state
            .store
            .file_protection(&f.session.scope)
            .await
            .unwrap()
            .rules
            .len(),
        1
    );
    let messages = f
        .state
        .store
        .list_messages(&f.session.scope, &f.session.id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "/protect private.txt");
    assert!(
        messages[1]
            .content
            .as_str()
            .unwrap()
            .contains("Protected locally")
    );
    let events = f
        .state
        .store
        .list_events(&f.session.scope.team_id, 0, 100)
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == "turn.completed" && event.payload["local"] == true)
    );
    assert!(
        !events
            .iter()
            .any(|event| event.kind.starts_with("privacy.request."))
    );
    let confirmation = events
        .iter()
        .find(|event| event.kind == "model.delta")
        .expect("live local confirmation");
    assert_eq!(confirmation.payload["local"], true);
    assert_eq!(confirmation.payload["item_id"], messages[1].id.0);
    assert_eq!(confirmation.payload["text"], messages[1].content);
    let completed = events
        .iter()
        .find(|event| event.kind == "turn.completed")
        .unwrap();
    assert_eq!(completed.payload["item_id"], messages[1].id.0);
    assert_eq!(completed.payload["model_calls"], 0);
}

#[tokio::test]
async fn protection_history_uses_originating_turn_and_fork_retry_do_not_reintroduce_it() {
    let f = Fixture::new().await;
    let old = user_turn(&f, "OLD_USER_SECRET").await;
    complete(&f, &old).await;
    let policy = f.protect().await;
    let late = f
        .state
        .store
        .append_turn_message(
            &old.scope,
            &f.session.id,
            &old.id,
            "assistant",
            serde_json::json!("LATE_OLD_ASSISTANT_SECRET"),
        )
        .await
        .unwrap();
    assert!(late.created_at > policy.changed_at.unwrap());
    let fresh = user_turn(&f, "FRESH_USER_CONTENT").await;
    complete(&f, &fresh).await;
    let mut history = f
        .state
        .store
        .list_messages(&old.scope, &f.session.id)
        .await
        .unwrap();
    protection::fresh_history(&f.state, &old.scope, &f.session.id, &mut history)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].turn_id, fresh.id);
    let (status, body) = http(
        &f.state,
        "POST",
        &format!("/v1/sessions/{}/fork", f.session.id.0),
        Some(serde_json::json!({"scope":old.scope})),
        Some("test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let fork: Session = serde_json::from_value(body).unwrap();
    let branch_history = f
        .state
        .store
        .list_messages(&old.scope, &fork.id)
        .await
        .unwrap();
    assert_eq!(branch_history.len(), 1);
    assert_eq!(branch_history[0].content, "FRESH_USER_CONTENT");
    let (status, body) = http(
        &f.state,
        "POST",
        &format!("/v1/turns/{}/retry", old.id.0),
        Some(serde_json::json!({"scope":old.scope})),
        Some("test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("File protection changed"));
    // Removing the final rule still must not restore the old transcript.
    f.state
        .store
        .replace_file_protection(&old.scope, policy.revision, vec![])
        .await
        .unwrap();
    let mut history = f
        .state
        .store
        .list_messages(&old.scope, &f.session.id)
        .await
        .unwrap();
    protection::fresh_history(&f.state, &old.scope, &f.session.id, &mut history)
        .await
        .unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn protection_title_generation_filters_old_question_and_rejects_stale_answer() {
    let f = Fixture::new().await;
    let old = user_turn(&f, "OLD_TITLE_QUESTION_SECRET").await;
    complete(&f, &old).await;
    let session = f.state.store.get_session(&f.session.id).await.unwrap();
    f.state
        .store
        .update_automatic_session_title(&old.scope, &session, "Pending conversation", false)
        .await
        .unwrap()
        .unwrap();
    f.protect().await;
    let fresh = user_turn(&f, "FRESH_TITLE_QUESTION").await;
    complete(&f, &fresh).await;
    let provider = Arc::new(CaptureProvider::default());
    maybe_generate_session_title(
        f.state.clone(),
        provider.clone(),
        &old,
        "fixture",
        "OLD_TITLE_ANSWER_SECRET",
    )
    .await;
    assert!(provider.requests.lock().unwrap().is_empty());
    maybe_generate_session_title(
        f.state.clone(),
        provider.clone(),
        &fresh,
        "fixture",
        "FRESH_TITLE_ANSWER",
    )
    .await;
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let sent = serde_json::to_string(&requests[0].messages).unwrap();
    assert!(sent.contains("FRESH_TITLE_QUESTION"));
    assert!(sent.contains("FRESH_TITLE_ANSWER"));
    assert!(!sent.contains("OLD_TITLE"));
}

#[tokio::test]
async fn protection_model_request_excludes_instructions_editor_memory_and_old_history() {
    let f = Fixture::new().await;
    let workspace = f.directory.path().join("project");
    std::fs::write(workspace.join("AGENTS.md"), "AGENTS_SECRET_SENTINEL").unwrap();
    f.state
        .store
        .upsert_editor_context(
            &f.session.id,
            s_code_protocol::UpdateEditorContext {
                scope: f.session.scope.clone(),
                protocol_version: s_code_protocol::IDE_PROTOCOL_VERSION.into(),
                client_instance_id: Id("editor".into()),
                workspace_uri: f.session.workspace_uri.clone(),
                active_document: Some(s_code_protocol::EditorDocument {
                    uri: url::Url::from_file_path(workspace.join("private.txt"))
                        .unwrap()
                        .to_string(),
                    language_id: "plaintext".into(),
                    version: 1,
                    text: Some("EDITOR_SECRET_SENTINEL".into()),
                }),
                selection: None,
                diagnostics: vec![],
            },
        )
        .await
        .unwrap();
    f.state
        .store
        .create_team_knowledge(s_code_protocol::CreateTeamKnowledgeItem {
            scope: f.session.scope.clone(),
            source_uri: "memory://user/private".into(),
            title: "Cached private memory".into(),
            content: "MEMORY_SECRET_SENTINEL".into(),
            version: "1".into(),
            trust_level: "user-provided".into(),
            permission: "team".into(),
            valid_until: None,
        })
        .await
        .unwrap();
    let old = user_turn(&f, "HISTORY_SECRET_SENTINEL").await;
    complete(&f, &old).await;
    // Prove the fixtures are actual context sources before activating the rule.
    let before = collect_context_items(
        &f.state,
        &f.session.scope,
        &f.session.id,
        &f.session.workspace_uri,
        Some("hello"),
    )
    .await
    .unwrap();
    let before = before
        .iter()
        .map(|item| item.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for marker in [
        "AGENTS_SECRET_SENTINEL",
        "EDITOR_SECRET_SENTINEL",
        "MEMORY_SECRET_SENTINEL",
    ] {
        assert!(before.contains(marker), "fixture missing {marker}");
    }
    f.protect().await;
    let fresh = user_turn(&f, "Fresh safe prompt").await;
    let provider = Arc::new(CaptureProvider::default());
    run_turn_with_step_inputs(
        f.state.clone(),
        provider.clone(),
        fresh.clone(),
        CancellationToken::new(),
        ToolProfile::Default,
        None,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        f.state
            .store
            .get_turn(&fresh.scope, &fresh.id)
            .await
            .unwrap()
            .status,
        TurnStatus::Completed
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let sent = serde_json::to_string(&request.messages).unwrap();
    assert!(sent.contains("Fresh safe prompt"));
    for marker in [
        "AGENTS_SECRET_SENTINEL",
        "EDITOR_SECRET_SENTINEL",
        "MEMORY_SECRET_SENTINEL",
        "HISTORY_SECRET_SENTINEL",
        "FILE_SECRET_SENTINEL",
    ] {
        assert!(!sent.contains(marker), "leaked {marker}");
    }
    assert!(!request.tools.is_empty());
    for tool in &request.tools {
        assert!(
            matches!(
                tool.name.as_str(),
                "read_file"
                    | "list_files"
                    | "search_text"
                    | "apply_patch"
                    | "start_work"
                    | "execute"
                    | "update_plan"
                    | "request_user_input"
                    | "report_review_findings"
            ),
            "unrestricted tool: {}",
            tool.name
        );
    }
    assert!(!request.tools.iter().any(|tool| matches!(
        tool.name.as_str(),
        "run_command" | "git_diff" | "terminal" | "get_goal"
    )));
}

#[tokio::test]
async fn protection_rejects_stale_question_resume_before_checkpoint_or_model_use() {
    let f = Fixture::new().await;
    let old = user_turn(&f, "OLD_RESUME_SECRET").await;
    for status in [
        TurnStatus::PreparingContext,
        TurnStatus::CallingModel,
        TurnStatus::AwaitingInput,
    ] {
        f.state
            .store
            .update_turn(
                &old.scope,
                &old.id,
                status,
                Some(&serde_json::json!({"old_checkpoint":"OLD_CHECKPOINT_SECRET"})),
                None,
            )
            .await
            .unwrap();
    }
    f.protect().await;
    let question = QuestionRequest {
        id: Id("question".into()),
        session_id: f.session.id.clone(),
        turn_id: old.id.clone(),
        item_id: Id("item".into()),
        questions: vec![],
        allow_other: true,
        requested_by: Id("assistant".into()),
        requested_at: old.started_at,
        expires_at: None,
        status: QuestionStatus::Answered,
        answers: vec![],
        answered_by: Some(old.scope.actor_id.clone()),
        answered_at: Some(Utc::now()),
        revision: 1,
    };
    let provider = Arc::new(CaptureProvider::default());
    let state = f.state.clone().with_model_provider(provider.clone());
    let result = maybe_resume_question(state, &old.scope, &question).await;
    assert!(
        matches!(result, Err(ApiError::Conflict(message)) if message.contains("File protection changed"))
    );
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn protection_http_activation_waits_for_dispatch_and_blocks_later_stale_requests() {
    let f = Fixture::new().await;
    let turn = user_turn(&f, "dispatch before protection").await;
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let provider = Arc::new(CaptureProvider {
        release: Some(release.clone()),
        ..Default::default()
    });
    let observed = privacy::ObservedProvider {
        inner: provider.clone(),
        state: f.state.clone(),
        turn: turn.clone(),
        purpose: "agent",
    };
    let request = || ModelRequest {
        model: "fixture".into(),
        temperature: 0.0,
        messages: vec![ModelMessage {
            role: "user".into(),
            content: serde_json::json!("DISPATCH_SECRET_SENTINEL"),
        }],
        tools: vec![],
        max_output_tokens: 32,
        routing: None,
    };
    let outgoing = request();
    let dispatch = tokio::spawn(async move { observed.stream(outgoing).await.is_ok() });
    tokio::time::timeout(Duration::from_secs(2), provider.entered.notified())
        .await
        .unwrap();
    let state = f.state.clone();
    let uri = f.protections();
    let scope = f.session.scope.clone();
    let mut activation = tokio::spawn(async move {
        http(
            &state,
            "POST",
            &uri,
            Some(serde_json::json!({"scope":scope,"path":"private.txt","expected_revision":0})),
            Some("test-token"),
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut activation)
            .await
            .is_err()
    );
    assert_eq!(
        f.state
            .store
            .file_protection(&f.session.scope)
            .await
            .unwrap()
            .revision,
        0
    );
    release.add_permits(1);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), dispatch)
            .await
            .unwrap()
            .unwrap()
    );
    let (status, body) = tokio::time::timeout(Duration::from_secs(2), activation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.state
            .store
            .file_protection(&f.session.scope)
            .await
            .unwrap()
            .revision,
        1
    );
    let stale = privacy::ObservedProvider {
        inner: provider.clone(),
        state: f.state.clone(),
        turn,
        purpose: "session_title",
    };
    let rejected = tokio::time::timeout(Duration::from_secs(2), stale.stream(request()))
        .await
        .unwrap();
    assert!(rejected.is_err());
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "stale request reached provider after protection acknowledged"
    );
}

#[tokio::test]
async fn protection_manual_compaction_cannot_repackage_old_history() {
    let f = Fixture::new().await;
    let old = user_turn(&f, &"OLD_COMPACTION_SECRET ".repeat(3000)).await;
    complete(&f, &old).await;
    f.protect().await;
    let uri = format!("/v1/sessions/{}/compact", f.session.id.0);
    let (status, body) = http(
        &f.state,
        "POST",
        &uri,
        Some(serde_json::json!({"scope": f.session.scope})),
        Some("test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("File protection changed"));
    let fresh = user_turn(&f, "Fresh small conversation").await;
    complete(&f, &fresh).await;
    let (status, body) = http(
        &f.state,
        "POST",
        &uri,
        Some(serde_json::json!({"scope": f.session.scope})),
        Some("test-token"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["omitted_messages"], 0);
    assert!(body["before_tokens"].as_u64().unwrap() < 100);
    let mut history = f
        .state
        .store
        .list_messages(&f.session.scope, &f.session.id)
        .await
        .unwrap();
    protection::fresh_history(&f.state, &f.session.scope, &f.session.id, &mut history)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "Fresh small conversation");
}

#[tokio::test]
async fn protection_retry_rechecks_origin_after_waiting_for_activation() {
    let f = Fixture::new().await;
    let old = user_turn(&f, "RETRY_ORIGIN_SECRET").await;
    complete(&f, &old).await;
    let gate = f.state.store.protection_gate(&old.scope);
    let dispatch = gate.read().await;
    let state = f.state.clone();
    let uri = f.protections();
    let scope = old.scope.clone();
    let mut activation = tokio::spawn(async move {
        http(
            &state,
            "POST",
            &uri,
            Some(serde_json::json!({"scope":scope,"path":"private.txt","expected_revision":0})),
            Some("test-token"),
        )
        .await
    });
    // A queued writer prevents further readers. Establish that exact ordering
    // before admitting the retry, whose first unguarded policy read is still 0.
    tokio::time::timeout(Duration::from_secs(2), async {
        while gate.try_read().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.state
            .store
            .file_protection(&old.scope)
            .await
            .unwrap()
            .revision,
        0
    );
    let provider = Arc::new(CaptureProvider::default());
    let state = f.state.clone().with_model_provider(provider.clone());
    let session_id = f.session.id.clone();
    let scope = old.scope.clone();
    let mut retry = tokio::spawn(async move {
        start_agent_turn(
            state,
            session_id,
            scope,
            protection::TurnContent {
                value: serde_json::json!("RETRY_ORIGIN_SECRET"),
                origin_started_at: Some(old.started_at),
            },
            vec![],
            ToolProfile::Default,
            false,
        )
        .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut retry)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut activation)
            .await
            .is_err()
    );
    drop(dispatch);
    let (status, body) = tokio::time::timeout(Duration::from_secs(2), activation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = tokio::time::timeout(Duration::from_secs(2), retry)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(ApiError::Conflict(message)) if message.contains("retry predates a privacy reset"))
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    assert_eq!(
        f.state
            .store
            .list_turns(&f.session.scope, &f.session.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn protection_api_rejects_attachments_and_inline_multimodal_content_before_dispatch() {
    let f = Fixture::new().await;
    f.protect().await;
    let provider = Arc::new(CaptureProvider::default());
    let state = f.state.clone().with_model_provider(provider.clone());
    let uri = format!("/v1/sessions/{}/turns", f.session.id.0);
    for (content, attachments) in [
        (
            serde_json::json!("Read this attachment"),
            serde_json::json!(["attachment-private"]),
        ),
        (
            serde_json::json!([{"type":"image_url","image_url":{"url":"data:image/png;base64,UFJJVkFURQ=="}}]),
            serde_json::json!([]),
        ),
        (
            serde_json::json!({"text":"PRIVATE_OBJECT_CONTENT"}),
            serde_json::json!([]),
        ),
    ] {
        let (status, body) = http(&state, "POST", &uri, Some(serde_json::json!({"scope":f.session.scope,"content":content,"attachment_ids":attachments,"generate_title":false})), Some("test-token")).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body.to_string().contains("Attachments cannot be sent"));
    }
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(
        f.state
            .store
            .list_turns(&f.session.scope, &f.session.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        f.state
            .store
            .list_messages(&f.session.scope, &f.session.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn protection_central_dispatch_rejects_structured_user_content_from_any_ingress() {
    let f = Fixture::new().await;
    f.protect().await;
    let fresh = user_turn(&f, "Fresh durable turn").await;
    let provider = Arc::new(CaptureProvider::default());
    let observed = privacy::ObservedProvider {
        inner: provider.clone(),
        state: f.state.clone(),
        turn: fresh,
        purpose: "durable_agent",
    };
    for content in [
        serde_json::json!([{"type":"image_url","image_url":{"url":"data:image/png;base64,UFJJVkFURQ=="}}]),
        serde_json::json!({"text":"UNVERIFIED_DURABLE_SECRET"}),
    ] {
        let result = observed
            .stream(ModelRequest {
                model: "fixture".into(),
                temperature: 0.0,
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content,
                }],
                tools: vec![],
                max_output_tokens: 32,
                routing: None,
            })
            .await;
        assert!(
            matches!(result, Err(GatewayError::Provider(message)) if message.contains("structured content"))
        );
    }
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "structured content bypassed normal admission through the central provider wrapper"
    );
}

#[tokio::test]
async fn protection_central_dispatch_filters_tools_reintroduced_by_context_changes() {
    let f = Fixture::new().await;
    f.protect().await;
    let fresh = user_turn(&f, "Fresh safe tool request").await;
    let provider = Arc::new(CaptureProvider::default());
    let observed = privacy::ObservedProvider {
        inner: provider.clone(),
        state: f.state.clone(),
        turn: fresh,
        purpose: "agent",
    };
    let definition = |name: &str| s_code_model_gateway::ToolDefinition {
        name: name.into(),
        description: format!("Fixture {name}"),
        parameters: serde_json::json!({"type":"object","properties":{}}),
    };
    // First dispatch is already restricted; the following dispatch simulates a
    // ContextChanged path replacing tool definitions with the full Work set.
    for names in [
        vec!["read_file"],
        vec![
            "read_file",
            "search_text",
            "run_command",
            "git_diff",
            "mcp_private_server",
            "terminal",
            "get_goal",
        ],
    ] {
        let result = observed
            .stream(ModelRequest {
                model: "fixture".into(),
                temperature: 0.0,
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: serde_json::json!("Safe text"),
                }],
                tools: names.into_iter().map(definition).collect(),
                max_output_tokens: 32,
                routing: None,
            })
            .await;
        assert!(result.is_ok());
    }
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_file"]
    );
    assert_eq!(
        requests[1]
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_file", "search_text"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn protection_terminal_abort_retains_gate_until_child_group_is_stopped() {
    // Exercise cancellation immediately after synchronous launch and after a
    // descendant is demonstrably running. Neither may release protection while
    // the process group can still access local files.
    for wait_for_child in [false, true] {
        let f = Fixture::new().await;
        let workspace = f.directory.path().join("project");
        let started = workspace.join("terminal-started");
        let late = workspace.join("terminal-late");
        let gate = f.state.store.protection_gate(&f.session.scope);
        let dispatch = gate.clone().read_owned().await;
        let spec = BackgroundTerminalSpec {
            session_id:f.session.id.clone(), program:"/bin/sh".into(),
            args:vec!["-c".into(), "(trap '' HUP TERM; printf ready > terminal-started; sleep 0.4; printf late > terminal-late) & wait".into()],
            environment_handles:BTreeMap::new(), working_directory_uri:f.session.workspace_uri.clone(), rows:24,cols:80,max_runtime_seconds:10,
        };
        let identity = host_directory_identity_sha256(&workspace).unwrap();
        let (handle, events) =
            launch_background_terminal(&spec, &identity, Some(dispatch)).unwrap();
        assert!(
            gate.try_write().is_err(),
            "terminal failed to retain the synchronously transferred protection guard"
        );
        if wait_for_child {
            tokio::time::timeout(Duration::from_secs(2), async {
                while !started.exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        // Simulate the request disappearing before runtime registration/event
        // draining: no application task remains to own or stop this terminal.
        drop(handle);
        drop(events);
        let _activation = tokio::time::timeout(Duration::from_secs(2), gate.write())
            .await
            .expect("abandoned terminal did not release protection after shutdown");
        assert!(
            !late.exists(),
            "child wrote private data before protection could activate"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !late.exists(),
            "descendant survived after protection activation acquired the exclusive gate"
        );
    }
}
