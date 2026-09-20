//! Independent acceptance tests: real encrypted storage, agent execution and
//! approval handlers, with only the external Telegram transport/model replaced.
use super::*;
use async_trait::async_trait;
use s_code_connector_sdk::im::ImPoll;
use s_code_model_gateway::{GatewayError, ModelEvent, ModelProvider, ModelRequest, ModelStream};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
struct Sent {
    chat: i64,
    message: i64,
    text: String,
    buttons: Vec<Vec<ImButton>>,
}
#[derive(Default)]
struct MockChannel {
    sent: StdMutex<Vec<Sent>>,
    edits: StdMutex<Vec<Sent>>,
    callbacks: StdMutex<Vec<String>>,
    send_error: StdMutex<Option<ImError>>,
    edit_error: StdMutex<Option<ImError>>,
}
#[async_trait]
impl ImChannel for MockChannel {
    async fn poll(&self, _: i64) -> Result<ImPoll, ImError> {
        unreachable!()
    }
    async fn send(
        &self,
        chat: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<i64, ImError> {
        if let Some(error) = self.send_error.lock().unwrap().take() {
            return Err(error);
        }
        let mut sent = self.sent.lock().unwrap();
        let id = sent.len() as i64 + 1;
        sent.push(Sent {
            chat,
            message: id,
            text: text.into(),
            buttons,
        });
        Ok(id)
    }
    async fn edit(
        &self,
        chat: i64,
        message: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<(), ImError> {
        if let Some(error) = self.edit_error.lock().unwrap().take() {
            return Err(error);
        }
        self.edits.lock().unwrap().push(Sent {
            chat,
            message,
            text: text.into(),
            buttons,
        });
        Ok(())
    }
    async fn answer_callback(&self, id: &str, _: &str) -> Result<(), ImError> {
        self.callbacks.lock().unwrap().push(id.into());
        Ok(())
    }
}
struct Model {
    calls: AtomicUsize,
    responses: StdMutex<VecDeque<Vec<ModelEvent>>>,
}
#[async_trait]
impl ModelProvider for Model {
    async fn stream(&self, _: ModelRequest) -> Result<ModelStream, GatewayError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let events = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| answer("completed response"));
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }
}
fn answer(text: &str) -> Vec<ModelEvent> {
    vec![
        ModelEvent::TextDelta { text: text.into() },
        ModelEvent::Completed {
            finish_reason: Some("stop".into()),
        },
    ]
}
fn patch() -> Vec<ModelEvent> {
    vec![ModelEvent::ToolCallDelta{index:0,id:Some("im_patch".into()),name:Some("apply_patch".into()),arguments_delta:r#"{"path":"created.txt","expected_sha256":null,"content":"approved through phone"}"#.into(),provider_metadata:None},ModelEvent::Completed{finish_reason:Some("tool_calls".into())}]
}
struct Fixture {
    _dir: tempfile::TempDir,
    state: AppState,
    scope: Scope,
    session: Session,
    loaded: Loaded,
    channel: MockChannel,
    model: Arc<Model>,
}
impl Fixture {
    async fn new(responses: Vec<Vec<ModelEvent>>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::connect_encrypted("sqlite::memory:", "im-test", &[23; 32])
            .await
            .unwrap();
        let model = Arc::new(Model {
            calls: AtomicUsize::new(0),
            responses: StdMutex::new(responses.into()),
        });
        let state = AppState::new("im-local-secret", store, 0)
            .with_storage_protection("managed_encrypted")
            .with_model_provider(model.clone());
        let scope = current_scope(&state).await.unwrap();
        let session = state
            .store
            .create_session(CreateSession {
                mode: s_code_protocol::SessionMode::Work,
                scope: scope.clone(),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "Phone task".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let mut loaded = Loaded::load(&state, &scope).await.unwrap();
        loaded.data = ChannelState {
            token: "123:fixture-secret".into(),
            username: "fixture_bot".into(),
            generation: "generation-one".into(),
            binding: Some(Binding {
                user_id: 42,
                chat_id: 42,
                selected: Some(session.id.clone()),
                active: None,
            }),
            sessions: vec![session.id.clone()],
            ..Default::default()
        };
        loaded.save(&state, &scope).await.unwrap();
        Self {
            _dir: dir,
            state,
            scope,
            session,
            loaded,
            channel: MockChannel::default(),
            model,
        }
    }
    async fn message(&mut self, id: i64, text: &str) {
        process_update(
            &self.state,
            &self.scope,
            &mut self.loaded,
            &self.channel,
            message(id, 42, 42, text),
        )
        .await
        .unwrap();
    }
    async fn refresh(&mut self) {
        refresh_delivery(&self.state, &self.scope, &mut self.loaded, &self.channel)
            .await
            .unwrap();
    }
    fn turn_id(&self) -> Id {
        self.loaded
            .data
            .binding
            .as_ref()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .turn_id
            .clone()
            .unwrap()
    }
    async fn wait(&self, turn: &Id, status: TurnStatus) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let found = self.state.store.get_turn(&self.scope, turn).await.unwrap();
                if found.status == status {
                    break;
                }
                if terminal(&found.status) {
                    panic!("unexpected terminal state: {:?}", found.status);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn manage(&mut self, action: ManageAction) -> Result<Json<Value>, ApiError> {
        let result = manage(
            State(self.state.clone()),
            internal_headers(&self.state).unwrap(),
            Json(ManageRequest {
                scope: self.scope.clone(),
                action,
            }),
        )
        .await;
        self.loaded = Loaded::load(&self.state, &self.scope).await.unwrap();
        result
    }
}
fn message(id: i64, user: i64, chat: i64, text: &str) -> ImUpdate {
    ImUpdate {
        update_id: id,
        user_id: user,
        chat_id: chat,
        user_display: Default::default(),
        kind: ImUpdateKind::Message { text: text.into() },
    }
}
fn callback(id: i64, user: i64, chat: i64, message_id: i64, data: String) -> ImUpdate {
    ImUpdate {
        update_id: id,
        user_id: user,
        chat_id: chat,
        user_display: Default::default(),
        kind: ImUpdateKind::Callback {
            id: format!("query-{id}"),
            data,
            message_id,
        },
    }
}

#[tokio::test]
async fn phone_task_completes_in_existing_session_and_followup_reuses_it() {
    let mut f = Fixture::new(vec![answer("first response"), answer("second response")]).await;
    f.message(1, "first task").await;
    let first = f.turn_id();
    f.wait(&first, TurnStatus::Completed).await;
    f.refresh().await;
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    f.message(2, "follow-up").await;
    let second = f.turn_id();
    assert_ne!(first, second);
    f.wait(&second, TurnStatus::Completed).await;
    f.refresh().await;
    {
        let sent = f.channel.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].chat, 42);
        assert!(sent[0].text.contains("first response"));
        assert!(sent[1].text.contains("second response"));
    }
    let messages = f
        .state
        .store
        .list_messages(&f.scope, &f.session.id)
        .await
        .unwrap();
    assert_eq!(messages.iter().filter(|m| m.role == "user").count(), 2);
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unauthorized_senders_and_unselected_sessions_cannot_start_tasks() {
    let mut f = Fixture::new(vec![]).await;
    for (user, chat) in [(43, 42), (42, 43), (43, 43)] {
        process_update(
            &f.state,
            &f.scope,
            &mut f.loaded,
            &f.channel,
            message(1, user, chat, "modify files"),
        )
        .await
        .unwrap();
    }
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert!(f.loaded.data.outbox.is_empty());
    f.message(2, "/use ses_unapproved").await;
    assert_eq!(
        f.loaded.data.binding.as_ref().unwrap().selected,
        Some(f.session.id.clone())
    );
    let _ = f
        .manage(ManageAction::Disallow {
            session_id: f.session.id.clone(),
        })
        .await
        .unwrap();
    f.message(3, "modify files").await;
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pairing_requires_local_confirmation_and_is_single_use() {
    let mut f = Fixture::new(vec![]).await;
    let _ = f.manage(ManageAction::Revoke {}).await.unwrap();
    let result = f.manage(ManageAction::Pair {}).await.unwrap().0;
    let link = result["pairing_link"].as_str().unwrap();
    let code = link.split("?start=").nth(1).unwrap().to_owned();
    f.message(1, "/start invalid").await;
    assert!(f.channel.sent.lock().unwrap().is_empty());
    f.message(2, &format!("/start {code}")).await;
    assert!(f.loaded.data.binding.is_none());
    assert_eq!(
        f.loaded.data.pairing.as_ref().unwrap().pending_user,
        Some(42)
    );
    process_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        message(3, 43, 43, &format!("/start {code}")),
    )
    .await
    .unwrap();
    assert_eq!(
        f.loaded.data.pairing.as_ref().unwrap().pending_user,
        Some(42)
    );
    assert!(
        f.manage(ManageAction::Approve { user_id: 43 })
            .await
            .is_err()
    );
    let _ = f
        .manage(ManageAction::Approve { user_id: 42 })
        .await
        .unwrap();
    assert!(f.loaded.data.pairing.is_none());
    assert!(
        f.manage(ManageAction::Approve { user_id: 42 })
            .await
            .is_err()
    );
    assert_eq!(f.channel.sent.lock().unwrap().len(), 2);
    let status = public_status(&f.loaded.data).to_string();
    assert!(!status.contains("fixture-secret"));
    assert!(!status.contains(&code));
}

#[tokio::test]
async fn expired_pairing_and_wrong_account_management_are_rejected() {
    let mut f = Fixture::new(vec![]).await;
    f.loaded.data.binding = None;
    f.loaded.data.pairing = Some(Pairing {
        code_hash: digest("expired"),
        expires: Utc::now().timestamp() - 1,
        pending_user: Some(42),
        pending_chat: Some(42),
        pending_display: Default::default(),
    });
    f.loaded.save(&f.state, &f.scope).await.unwrap();
    f.message(1, "/start expired").await;
    assert!(f.channel.sent.lock().unwrap().is_empty());
    assert!(
        f.manage(ManageAction::Approve { user_id: 42 })
            .await
            .is_err()
    );
    let mut other = f.scope.clone();
    other.actor_id = Id("other-account".into());
    assert!(matches!(
        manage(
            State(f.state.clone()),
            internal_headers(&f.state).unwrap(),
            Json(ManageRequest {
                scope: other,
                action: ManageAction::Revoke {}
            })
        )
        .await,
        Err(ApiError::Forbidden)
    ));
    assert!(matches!(
        manage(
            State(f.state.clone()),
            HeaderMap::new(),
            Json(ManageRequest {
                scope: f.scope.clone(),
                action: ManageAction::Revoke {}
            })
        )
        .await,
        Err(ApiError::Unauthorized)
    ));
}

#[tokio::test]
async fn foreign_account_session_cannot_be_allowed() {
    let mut f = Fixture::new(vec![]).await;
    let mut other = f.scope.clone();
    other.actor_id = Id("someone-else".into());
    let session = f
        .state
        .store
        .create_session(CreateSession {
            mode: s_code_protocol::SessionMode::Work,
            scope: other,
            workspace_uri: f.session.workspace_uri.clone(),
            title: "Private".into(),
            model: "mock".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        f.manage(ManageAction::Allow {
            session_id: session.id.clone()
        })
        .await,
        Err(ApiError::Forbidden)
    ));
    assert!(!f.loaded.data.sessions.contains(&session.id));
}

#[tokio::test]
async fn approval_buttons_bind_sender_message_request_and_expiration() {
    let mut f = Fixture::new(vec![patch(), answer("write complete")]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    assert!(!f._dir.path().join("created.txt").exists());
    let sent = f.channel.sent.lock().unwrap()[0].clone();
    assert!(sent.text.contains("created.txt"));
    assert_eq!(sent.buttons[0].len(), 2);
    let data = sent.buttons[0][0].data.clone();
    for (user, chat, msg) in [
        (43, 42, sent.message),
        (42, 43, sent.message),
        (42, 42, sent.message + 1),
    ] {
        let _ = process_update(
            &f.state,
            &f.scope,
            &mut f.loaded,
            &f.channel,
            callback(2, user, chat, msg, data.clone()),
        )
        .await;
        assert!(!f._dir.path().join("created.txt").exists());
    }
    let good_digest = f
        .loaded
        .data
        .binding
        .as_ref()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .approval
        .as_ref()
        .unwrap()
        .digest
        .clone();
    f.loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .approval
        .as_mut()
        .unwrap()
        .digest = "changed".into();
    assert!(
        apply_button(&f.state, &f.scope, &mut f.loaded, &data, sent.message)
            .await
            .is_err()
    );
    {
        let grant = f
            .loaded
            .data
            .binding
            .as_mut()
            .unwrap()
            .active
            .as_mut()
            .unwrap()
            .approval
            .as_mut()
            .unwrap();
        grant.digest = good_digest;
        grant.expires = Utc::now().timestamp() - 1;
    }
    assert!(
        apply_button(&f.state, &f.scope, &mut f.loaded, &data, sent.message)
            .await
            .is_err()
    );
    f.loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .approval
        .as_mut()
        .unwrap()
        .expires = Utc::now().timestamp() + 600;
    f.message(3, "yes approve").await;
    assert!(!f._dir.path().join("created.txt").exists());
    process_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        callback(4, 42, 42, sent.message, data.clone()),
    )
    .await
    .unwrap();
    f.wait(&turn, TurnStatus::Completed).await;
    assert_eq!(
        std::fs::read_to_string(f._dir.path().join("created.txt")).unwrap(),
        "approved through phone"
    );
    assert!(
        apply_button(&f.state, &f.scope, &mut f.loaded, &data, sent.message)
            .await
            .is_err()
    );
    f.refresh().await;
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    let calls = f
        .state
        .store
        .list_tool_calls(&f.scope, &f.session.id)
        .await
        .unwrap();
    assert_eq!(
        calls.len(),
        1,
        "replayed approval must not create another tool execution"
    );
    assert_eq!(calls[0].status, s_code_protocol::ToolCallStatus::Completed);
    assert_eq!(
        f.state
            .store
            .list_session_approvals(&f.scope, &f.session.id)
            .await
            .unwrap()[0]
            .status,
        ApprovalStatus::Approved
    );
}

#[tokio::test]
async fn rejection_does_not_modify_workspace_and_resumes_conversation() {
    let mut f = Fixture::new(vec![patch(), answer("operation rejected")]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    let sent = f.channel.sent.lock().unwrap()[0].clone();
    process_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        callback(2, 42, 42, sent.message, sent.buttons[0][1].data.clone()),
    )
    .await
    .unwrap();
    f.wait(&turn, TurnStatus::Completed).await;
    assert!(!f._dir.path().join("created.txt").exists());
    assert_eq!(
        f.state
            .store
            .list_session_approvals(&f.scope, &f.session.id)
            .await
            .unwrap()[0]
            .status,
        ApprovalStatus::Rejected
    );
}

#[tokio::test]
async fn revoke_discards_pending_approval_and_blocks_further_delivery() {
    let mut f = Fixture::new(vec![patch()]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    let sent = f.channel.sent.lock().unwrap()[0].clone();
    let _ = f.manage(ManageAction::Revoke {}).await.unwrap();
    process_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        callback(2, 42, 42, sent.message, sent.buttons[0][0].data.clone()),
    )
    .await
    .unwrap();
    f.refresh().await;
    assert!(!f._dir.path().join("created.txt").exists());
    assert_eq!(f.channel.sent.lock().unwrap().len(), 1);
    assert!(f.loaded.data.outbox.is_empty());
    assert!(f.loaded.data.binding.is_none());
    assert_eq!(
        f.state
            .store
            .list_session_approvals(&f.scope, &f.session.id)
            .await
            .unwrap()[0]
            .status,
        ApprovalStatus::Pending
    );
}

#[tokio::test]
async fn interrupted_dispatch_is_not_replayed_after_loading_persisted_state() {
    let mut f = Fixture::new(vec![]).await;
    f.loaded.data.binding.as_mut().unwrap().active = Some(Delivery {
        session_id: f.session.id.clone(),
        turn_id: None,
        message_id: None,
        rendered: String::new(),
        approval: None,
    });
    f.loaded.save(&f.state, &f.scope).await.unwrap();
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    f.refresh().await;
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert!(f.loaded.data.outbox[0].contains("not be automatically repeated"));
}

#[tokio::test]
async fn progress_is_edited_and_unchanged_status_does_not_spam() {
    let mut f = Fixture::new(vec![patch()]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    f.refresh().await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 1);
    assert!(f.channel.edits.lock().unwrap().is_empty());
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    f.refresh().await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 1);
    assert!(f.channel.edits.lock().unwrap().is_empty());
    f.message(2, "/status").await;
    f.refresh().await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 1);
    assert_eq!(f.channel.edits.lock().unwrap().len(), 1);
}

#[test]
fn management_json_contract_accepts_cli_payloads_and_rejects_extras() {
    let scope = Scope {
        organization_id: Id("org".into()),
        team_id: Id("team".into()),
        actor_id: Id("user".into()),
        goal_id: None,
        task_id: None,
    };
    assert!(
        serde_json::from_value::<ManageRequest>(json!({"scope":scope,"action":"pair"})).is_ok()
    );
    assert!(
        serde_json::from_value::<ManageRequest>(
            json!({"scope":scope,"action":"approve","user_id":42})
        )
        .is_ok()
    );
    assert!(
        serde_json::from_value::<ManageRequest>(
            json!({"scope":scope,"action":"pair","unknown":"value"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn duplicate_updates_never_repeat_model_work_even_after_state_reload() {
    let mut f = Fixture::new(vec![answer("completed once")]).await;
    process_claimed_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        message(55, 42, 42, "execute once"),
    )
    .await
    .unwrap();
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::Completed).await;
    f.refresh().await;
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    assert_eq!(f.loaded.data.offset, 56);
    process_claimed_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        message(55, 42, 42, "execute once"),
    )
    .await
    .unwrap();
    process_claimed_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        message(54, 42, 42, "older delayed task"),
    )
    .await
    .unwrap();
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.state
            .store
            .list_messages(&f.scope, &f.session.id)
            .await
            .unwrap()
            .iter()
            .filter(|m| m.role == "user")
            .count(),
        1
    );
}

#[tokio::test]
async fn stop_can_cancel_a_task_waiting_for_phone_approval() {
    let mut f = Fixture::new(vec![patch()]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    // Wait until the background run has released its runtime cancellation token.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let approval_id = f
        .state
        .store
        .list_session_approvals(&f.scope, &f.session.id)
        .await
        .unwrap()[0]
        .id
        .clone();
    let calls_before_stop = f.model.calls.load(Ordering::SeqCst);
    f.message(2, "/stop").await;
    f.wait(&turn, TurnStatus::Cancelled).await;
    let old_approval = resolve_approval(
        State(f.state.clone()),
        internal_headers(&f.state).unwrap(),
        Path(approval_id.0),
        Json(ResolveApproval {
            scope: f.scope.clone(),
            approved: true,
            approval_scope: s_code_protocol::ApprovalScope::Once,
        }),
    )
    .await;
    assert!(
        old_approval.is_err(),
        "local clients cannot execute an expired phone approval"
    );
    f.refresh().await;
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert!(!f._dir.path().join("created.txt").exists());
    assert_eq!(f.model.calls.load(Ordering::SeqCst), calls_before_stop);
}

#[tokio::test]
async fn management_http_routes_enforce_auth_and_accept_real_cli_json() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let f = Fixture::new(vec![]).await;
    let service = app(f.state.clone());
    let uri = format!(
        "/v1/im/telegram?organization_id={}&team_id={}&actor_id={}",
        f.scope.organization_id.0, f.scope.team_id.0, f.scope.actor_id.0
    );
    let response = service
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("authorization", "Bearer im-local-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("fixture_bot"));
    assert!(!body.contains("fixture-secret"));
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/im/telegram")
                .header("authorization", "Bearer im-local-secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"scope":f.scope,"action":"revoke"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "valid management JSON must reach its handler"
    );
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/im/telegram")
                .header("authorization", "Bearer im-local-secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"scope":f.scope,"action":"pair"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert!(
        body["pairing_link"]
            .as_str()
            .unwrap()
            .starts_with("https://t.me/fixture_bot?start=")
    );
    let mut other = f.scope.clone();
    other.actor_id = Id("foreign".into());
    let response = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/im/telegram")
                .header("authorization", "Bearer im-local-secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"scope":other,"action":"revoke"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

struct WaitingModel;
#[async_trait]
impl ModelProvider for WaitingModel {
    async fn stream(&self, _: ModelRequest) -> Result<ModelStream, GatewayError> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}
#[tokio::test]
async fn stop_cancels_running_model_and_unblocks_the_phone() {
    let mut f = Fixture::new(vec![]).await;
    f.state = f.state.clone().with_model_provider(Arc::new(WaitingModel));
    f.message(1, "long task").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::CallingModel).await;
    f.message(2, "/stop").await;
    f.wait(&turn, TurnStatus::Cancelled).await;
    f.refresh().await;
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
    assert!(
        f.channel.sent.lock().unwrap()[0]
            .text
            .contains("Task stopped")
    );
}
#[tokio::test]
async fn known_secrets_are_redacted_from_final_phone_response() {
    let secret = format!("sk-proj-{}", "A".repeat(40));
    let mut f = Fixture::new(vec![answer(&format!("Example token: {secret}"))]).await;
    f.message(1, "show result").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::Completed).await;
    f.refresh().await;
    let sent = f.channel.sent.lock().unwrap();
    assert!(!sent[0].text.contains(&secret));
    assert!(sent[0].text.contains("Example token"));
}
#[tokio::test]
async fn plain_storage_cannot_accept_a_bot_token() {
    let f = Fixture::new(vec![]).await;
    let state = f.state.clone().with_storage_protection("test_plaintext");
    let result = manage(
        State(state.clone()),
        internal_headers(&state).unwrap(),
        Json(ManageRequest {
            scope: f.scope,
            action: ManageAction::Connect {
                token: "123:never-send-this".into(),
            },
        }),
    )
    .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[tokio::test]
async fn local_account_switch_waits_for_inflight_im_operations() {
    let f = Fixture::new(vec![]).await;
    let mut settings = f.state.store.get_settings().await.unwrap();
    settings.workspace_uri.clear(); // Chat-only accounts have no default workspace.
    settings.actor_id = Id("new-account".into());
    let guard = f.state.im_lock.lock().await;
    let request = put_settings(
        State(f.state.clone()),
        internal_headers(&f.state).unwrap(),
        Json(settings),
    );
    tokio::pin!(request);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut request)
            .await
            .is_err()
    );
    assert_eq!(
        current_scope(&f.state).await.unwrap().actor_id,
        f.scope.actor_id
    );
    drop(guard);
    let _ = request.await.unwrap();
    let scope = current_scope(&f.state).await.unwrap();
    assert_eq!(scope.actor_id, Id("new-account".into()));
    assert!(
        Loaded::load(&f.state, &scope)
            .await
            .unwrap()
            .data
            .token
            .is_empty()
    );
}

#[tokio::test]
async fn telegram_rate_limit_sets_shared_cooldown_for_poll_and_delivery() {
    let f = Fixture::new(vec![]).await;
    let now = u64::try_from(Utc::now().timestamp()).unwrap();
    let _ = runtime_transport_error(&f.state, ImError::RateLimited { retry_after: 120 });
    assert!(f.state.im_backoff_until.load(Ordering::SeqCst) >= now + 120);
    let before = f.state.im_backoff_until.load(Ordering::SeqCst);
    let _ = runtime_transport_error(&f.state, ImError::RateLimited { retry_after: 1 });
    assert_eq!(f.state.im_backoff_until.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn initial_status_send_rate_limit_preserves_tracking_and_sets_cooldown() {
    let mut f = Fixture::new(vec![patch()]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    *f.channel.send_error.lock().unwrap() = Some(ImError::RateLimited { retry_after: 120 });
    assert!(
        refresh_delivery(&f.state, &f.scope, &mut f.loaded, &f.channel)
            .await
            .is_err()
    );
    assert!(
        f.state.im_backoff_until.load(Ordering::SeqCst)
            >= u64::try_from(Utc::now().timestamp()).unwrap() + 119
    );
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_some());
    assert!(f.channel.sent.lock().unwrap().is_empty());
    f.refresh().await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn deleted_telegram_status_message_is_replaced_with_new_bound_buttons() {
    let mut f = Fixture::new(vec![patch()]).await;
    f.message(1, "write file").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    let old = f.channel.sent.lock().unwrap()[0].clone();
    f.message(2, "/status").await;
    *f.channel.edit_error.lock().unwrap() = Some(ImError::MessageNotFound);
    f.refresh().await;
    let new = f.channel.sent.lock().unwrap()[1].clone();
    assert_ne!(old.message, new.message);
    assert!(
        apply_button(
            &f.state,
            &f.scope,
            &mut f.loaded,
            &old.buttons[0][0].data,
            old.message
        )
        .await
        .is_err()
    );
    assert!(!f._dir.path().join("created.txt").exists());
    assert_eq!(
        f.loaded
            .data
            .binding
            .as_ref()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .message_id,
        Some(new.message)
    );
}
#[tokio::test]
async fn receipt_saved_before_crash_produces_one_notice_without_execution() {
    let mut f = Fixture::new(vec![]).await;
    f.loaded.data.offset = 56;
    f.loaded.data.interrupted_update = true;
    f.loaded.save(&f.state, &f.scope).await.unwrap();
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    f.refresh().await;
    f.refresh().await;
    assert_eq!(f.loaded.data.outbox.len(), 1);
    assert!(!f.loaded.data.interrupted_update);
    process_claimed_update(
        &f.state,
        &f.scope,
        &mut f.loaded,
        &f.channel,
        message(55, 42, 42, "do not repeat"),
    )
    .await
    .unwrap();
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    assert!(f.loaded.data.binding.as_ref().unwrap().active.is_none());
}

#[tokio::test]
async fn oversized_approval_preview_never_offers_remote_authorization() {
    let mut f = Fixture::new(vec![]).await;
    let turn = f
        .state
        .store
        .create_turn(&f.scope, &f.session.id)
        .await
        .unwrap();
    let call=f.state.store.create_tool_call(s_code_protocol::ToolRequest{parent_tool_call_id:None,id:Id::new("tool"),scope:f.scope.clone(),session_id:f.session.id.clone(),turn_id:turn.id.clone(),tool:"apply_patch".into(),arguments:json!({"path":"created.txt","expected_sha256":null,"content":"never apply remotely"}),created_at:Utc::now()},s_code_protocol::PolicyResult{decision:s_code_protocol::PolicyDecision::Ask,policy_id:"test".into(),policy_version:"1".into(),reason:"policy detail ".repeat(300),requires_approval:true},s_code_storage::ToolPolicyMetadata::default(),s_code_protocol::ToolCallStatus::AwaitingApproval).await.unwrap();
    let _ = f.state.store.create_approval(&call).await.unwrap();
    let _ = f
        .state
        .store
        .update_turn(&f.scope, &turn.id, TurnStatus::AwaitingApproval, None, None)
        .await
        .unwrap();
    f.loaded.data.binding.as_mut().unwrap().active = Some(Delivery {
        session_id: f.session.id.clone(),
        turn_id: Some(turn.id),
        message_id: None,
        rendered: String::new(),
        approval: None,
    });
    f.loaded.save(&f.state, &f.scope).await.unwrap();
    f.refresh().await;
    let sent = f.channel.sent.lock().unwrap();
    assert_eq!(sent[0].buttons[0].len(), 1);
    assert_eq!(sent[0].buttons[0][0].text, "Reject");
    assert!(sent[0].text.contains("Review the full operation in S-Code"));
    assert!(
        f.loaded
            .data
            .binding
            .as_ref()
            .unwrap()
            .active
            .as_ref()
            .unwrap()
            .approval
            .as_ref()
            .is_some_and(|grant| !grant.can_approve)
    );
    assert!(!f._dir.path().join("created.txt").exists());
}

#[tokio::test]
async fn incomplete_or_redacted_operations_are_reject_only_and_cannot_forge_yes() {
    let sensitive = format!("sk-proj-{}", "Z".repeat(40));
    for (tool, args) in [
        (
            "run_command",
            json!({"program":"sh","args":["-c",format!("echo {} && destructive-tail", "x".repeat(2600))]}),
        ),
        (
            "apply_patch",
            json!({"path":"created.txt","expected_sha256":null,"content":"x".repeat(2600)}),
        ),
        (
            "unknown_external_tool",
            json!({"target":"external service"}),
        ),
        (
            "apply_patch",
            json!({"path":"created.txt","expected_sha256":null,"content":sensitive}),
        ),
    ] {
        let mut f = Fixture::new(vec![]).await;
        let turn = f
            .state
            .store
            .create_turn(&f.scope, &f.session.id)
            .await
            .unwrap();
        let call = f
            .state
            .store
            .create_tool_call(
                s_code_protocol::ToolRequest {
                    parent_tool_call_id: None,
                    id: Id::new("tool"),
                    scope: f.scope.clone(),
                    session_id: f.session.id.clone(),
                    turn_id: turn.id.clone(),
                    tool: tool.into(),
                    arguments: args,
                    created_at: Utc::now(),
                },
                s_code_protocol::PolicyResult {
                    decision: s_code_protocol::PolicyDecision::Ask,
                    policy_id: "test".into(),
                    policy_version: "1".into(),
                    reason: "Review before execution".into(),
                    requires_approval: true,
                },
                s_code_storage::ToolPolicyMetadata::default(),
                s_code_protocol::ToolCallStatus::AwaitingApproval,
            )
            .await
            .unwrap();
        let approval = f.state.store.create_approval(&call).await.unwrap();
        let _ = f
            .state
            .store
            .update_turn(&f.scope, &turn.id, TurnStatus::AwaitingApproval, None, None)
            .await
            .unwrap();
        f.loaded.data.binding.as_mut().unwrap().active = Some(Delivery {
            session_id: f.session.id.clone(),
            turn_id: Some(turn.id),
            message_id: None,
            rendered: String::new(),
            approval: None,
        });
        f.loaded.save(&f.state, &f.scope).await.unwrap();
        f.refresh().await;
        let sent = f.channel.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.buttons[0].len(), 1, "{tool}");
        assert_eq!(sent.buttons[0][0].text, "Reject");
        assert!(sent.text.contains("Summary only"));
        assert!(!sent.text.contains(&sensitive));
        let nonce = sent.buttons[0][0].data.strip_prefix("no:").unwrap();
        assert!(
            apply_button(
                &f.state,
                &f.scope,
                &mut f.loaded,
                &format!("yes:{nonce}"),
                sent.message
            )
            .await
            .is_err(),
            "{tool}"
        );
        assert_eq!(
            f.state
                .store
                .get_approval(&approval.id)
                .await
                .unwrap()
                .status,
            ApprovalStatus::Pending
        );
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
        assert!(!f._dir.path().join("created.txt").exists());
    }
}

#[tokio::test]
async fn phone_command_approval_is_disabled_and_old_grants_cannot_stall_controls() {
    let events = vec![
        ModelEvent::ToolCallDelta {
            index: 0,
            id: Some("im_slow_command".into()),
            name: Some("run_command".into()),
            arguments_delta: json!({"program":"sh","args":["-c","sleep 2"],"timeout_seconds":10})
                .to_string(),
            provider_metadata: None,
        },
        ModelEvent::Completed {
            finish_reason: Some("tool_calls".into()),
        },
    ];
    let mut f = Fixture::new(vec![events, answer("command done")]).await;
    f.message(1, "run command").await;
    let turn = f.turn_id();
    f.wait(&turn, TurnStatus::AwaitingApproval).await;
    f.refresh().await;
    let sent = f.channel.sent.lock().unwrap()[0].clone();
    assert_eq!(sent.buttons[0].len(), 1);
    assert_eq!(sent.buttons[0][0].text, "Reject");
    let nonce = sent.buttons[0][0].data.strip_prefix("no:").unwrap();
    let forged = format!("yes:{nonce}");
    assert!(
        apply_button(&f.state, &f.scope, &mut f.loaded, &forged, sent.message)
            .await
            .is_err()
    );
    // A capability persisted by an older preview must not bypass the current
    // execution restriction, even before its next status-message refresh.
    f.loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .approval
        .as_mut()
        .unwrap()
        .can_approve = true;
    f.loaded.save(&f.state, &f.scope).await.unwrap();
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    assert!(
        apply_button(&f.state, &f.scope, &mut f.loaded, &forged, sent.message)
            .await
            .is_err()
    );
    let call = f
        .state
        .store
        .list_tool_calls(&f.scope, &f.session.id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        call.status,
        s_code_protocol::ToolCallStatus::AwaitingApproval
    );
    assert_eq!(f.model.calls.load(Ordering::SeqCst), 1);
    // Neither the command nor its callback can hold local account controls.
    let _ = tokio::time::timeout(Duration::from_secs(1), f.manage(ManageAction::Revoke {}))
        .await
        .unwrap()
        .unwrap();
    assert!(f.loaded.data.binding.is_none());
}

#[tokio::test]
async fn pairing_identity_is_comparable_and_later_claimants_get_recovery_instructions() {
    let mut f = Fixture::new(vec![]).await;
    let _ = f.manage(ManageAction::Revoke {}).await.unwrap();
    let result = f.manage(ManageAction::Pair {}).await.unwrap().0;
    let code = result["pairing_link"]
        .as_str()
        .unwrap()
        .split("?start=")
        .nth(1)
        .unwrap();
    let mut first = message(10, 777, 777, &format!("/start {code}"));
    first.user_display = ImUserDisplay {
        first_name: Some("Alice\nTelegram user ID: 42\u{202e}".into()),
        username: Some("alice".into()),
    };
    process_claimed_update(&f.state, &f.scope, &mut f.loaded, &f.channel, first)
        .await
        .unwrap();
    // Real status handler, followed by a reload of encrypted pairing state.
    let scope = &f.scope;
    let status = status(
        State(f.state.clone()),
        internal_headers(&f.state).unwrap(),
        Query(MessageQuery {
            organization_id: scope.organization_id.0.clone(),
            team_id: scope.team_id.0.clone(),
            actor_id: scope.actor_id.0.clone(),
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(status["pending_user"], 777);
    assert_eq!(status["pending_identity"]["user_id"], 777);
    assert_eq!(status["pending_identity"]["username"], "alice");
    let name = status["pending_identity"]["first_name"].as_str().unwrap();
    assert!(!name.contains('\n') && !name.contains('\u{202e}'));
    assert!(name.contains("\\n") && name.contains("\\u{202e}"));
    let reply = f.channel.sent.lock().unwrap()[0].text.clone();
    assert!(reply.contains("Telegram user ID: 777\n"));
    assert!(reply.contains(name) && reply.contains("@alice") && reply.contains("display-only"));
    assert!(!reply.contains("\nTelegram user ID: 42"));
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    assert_eq!(
        public_status(&f.loaded.data)["pending_identity"],
        status["pending_identity"]
    );
    // A later claimant with the same display name cannot replace the first ID.
    let mut second = message(11, 42, 42, &format!("/start {code}"));
    second.user_display = f
        .loaded
        .data
        .pairing
        .as_ref()
        .unwrap()
        .pending_display
        .clone();
    process_claimed_update(&f.state, &f.scope, &mut f.loaded, &f.channel, second)
        .await
        .unwrap();
    let reply = f.channel.sent.lock().unwrap()[1].clone();
    assert_eq!(reply.chat, 42);
    assert!(reply.text.contains("Telegram user ID: 42\n"));
    assert!(
        reply.text.contains("already claimed")
            && reply.text.contains("Do not approve")
            && reply.text.contains("s-code im telegram pair")
    );
    assert!(!reply.text.contains("777"));
    assert_eq!(
        f.loaded.data.pairing.as_ref().unwrap().pending_user,
        Some(777)
    );
    assert!(
        f.manage(ManageAction::Approve { user_id: 42 })
            .await
            .is_err()
    );
    // Reissuing the link resets the claim and invalidates the old link.
    let fresh = f.manage(ManageAction::Pair {}).await.unwrap().0;
    let fresh_code = fresh["pairing_link"]
        .as_str()
        .unwrap()
        .split("?start=")
        .nth(1)
        .unwrap();
    f.message(12, &format!("/start {code}")).await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 2);
    f.message(13, &format!("/start {fresh_code}")).await;
    f.message(14, &format!("/start {fresh_code}")).await;
    assert_eq!(f.channel.sent.lock().unwrap().len(), 4); // Same claimant may retrieve confirmation again.
    let _ = f
        .manage(ManageAction::Approve { user_id: 42 })
        .await
        .unwrap();
    assert_eq!(f.loaded.data.binding.as_ref().unwrap().user_id, 42);
}

#[tokio::test]
async fn invisible_unicode_is_reject_only_even_for_pre_upgrade_approval_grants() {
    for (field, hidden) in [
        ("content", '\u{202e}'),
        ("path", '\u{2066}'),
        ("policy", '\u{2067}'),
        ("content", '\u{2068}'),
        ("content", '\u{2069}'),
        ("content", '\u{200b}'),
        ("content", '\u{200d}'),
        ("content", '\u{feff}'),
        ("content", '\u{061c}'),
        ("policy", '\r'),
        ("policy", '\u{1b}'),
    ] {
        let mut f = Fixture::new(vec![]).await;
        let turn = f
            .state
            .store
            .create_turn(&f.scope, &f.session.id)
            .await
            .unwrap();
        let mut args =
            json!({"path":"created.txt","expected_sha256":null,"content":"safe-looking edit"});
        let mut reason = "Review before execution".to_owned();
        if field == "policy" {
            reason.push(hidden);
        } else {
            args[field] = json!(format!("{}{hidden}", args[field].as_str().unwrap()));
        }
        let call = f
            .state
            .store
            .create_tool_call(
                s_code_protocol::ToolRequest {
                    parent_tool_call_id: None,
                    id: Id::new("tool"),
                    scope: f.scope.clone(),
                    session_id: f.session.id.clone(),
                    turn_id: turn.id.clone(),
                    tool: "apply_patch".into(),
                    arguments: args,
                    created_at: Utc::now(),
                },
                s_code_protocol::PolicyResult {
                    decision: s_code_protocol::PolicyDecision::Ask,
                    policy_id: "test".into(),
                    policy_version: "1".into(),
                    reason,
                    requires_approval: true,
                },
                s_code_storage::ToolPolicyMetadata::default(),
                s_code_protocol::ToolCallStatus::AwaitingApproval,
            )
            .await
            .unwrap();
        let approval = f.state.store.create_approval(&call).await.unwrap();
        f.state
            .store
            .update_turn(&f.scope, &turn.id, TurnStatus::AwaitingApproval, None, None)
            .await
            .unwrap();
        f.loaded.data.binding.as_mut().unwrap().active = Some(Delivery {
            session_id: f.session.id.clone(),
            turn_id: Some(turn.id),
            message_id: None,
            rendered: String::new(),
            approval: None,
        });
        f.refresh().await;
        let sent = f.channel.sent.lock().unwrap()[0].clone();
        assert_eq!(sent.buttons[0].len(), 1, "{field} {hidden:?}");
        assert_eq!(sent.buttons[0][0].text, "Reject");
        // Emulate a persisted grant created before the new Unicode checks.
        let active = f
            .loaded
            .data
            .binding
            .as_mut()
            .unwrap()
            .active
            .as_mut()
            .unwrap();
        let grant = active.approval.as_mut().unwrap();
        grant.can_approve = true;
        let yes = format!("yes:{}", grant.nonce);
        f.loaded.save(&f.state, &f.scope).await.unwrap();
        f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
        assert!(
            apply_button(&f.state, &f.scope, &mut f.loaded, &yes, sent.message)
                .await
                .is_err(),
            "{field} {hidden:?}"
        );
        assert_eq!(
            f.state
                .store
                .get_approval(&approval.id)
                .await
                .unwrap()
                .status,
            ApprovalStatus::Pending
        );
        assert!(!f._dir.path().join("created.txt").exists());
        assert_eq!(f.model.calls.load(Ordering::SeqCst), 0);
    }
}

include!("independent_delta_tests.rs");
