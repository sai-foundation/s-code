//! Conversation mode and the host-controlled workspace transition.
use super::*;
use s_code_protocol::SessionMode;

pub(super) fn validate_session_workspace(session: &Session) -> Result<(), ApiError> {
    if session.mode == SessionMode::Chat {
        if !session.workspace_uri.is_empty() {
            return Err(ApiError::BadRequest("Chat cannot have a workspace".into()));
        }
    } else {
        validate_workspace(&session.workspace_uri)?;
    }
    Ok(())
}

pub(super) fn create_managed_workspace(state: &AppState) -> Result<String, ApiError> {
    let root = state.managed_workspaces.as_deref().ok_or_else(|| {
        ApiError::BadRequest("Home directory is unavailable; choose a workspace explicitly".into())
    })?;
    if !root.is_absolute() {
        return Err(ApiError::BadRequest(
            "Managed workspace root must be absolute".into(),
        ));
    }
    // Never use an existing symlink as a workspace container. The private root
    // also prevents other OS accounts from replacing its newly created children.
    match std::fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ApiError::BadRequest(
                "Managed workspace root must be a real directory".into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            if let Err(error) = builder.create(root)
                && error.kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(ApiError::BadRequest(format!(
                    "Cannot create managed workspace root: {error}"
                )));
            }
            let metadata =
                std::fs::symlink_metadata(root).map_err(|e| ApiError::BadRequest(e.to_string()))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ApiError::BadRequest(
                    "Managed workspace root changed".into(),
                ));
            }
        }
        Err(error) => return Err(ApiError::BadRequest(error.to_string())),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(root)
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(ApiError::BadRequest(
                "Managed workspace root must be private (permissions 0700)".into(),
            ));
        }
    }
    let root = root
        .canonicalize()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let root_uri = url::Url::from_directory_path(&root)
        .map_err(|_| ApiError::BadRequest("Invalid workspace root".into()))?
        .to_string();
    validate_workspace(&root_uri)?;
    let path = root.join(Id::new("work").0);
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&path)
        .map_err(|e| ApiError::BadRequest(format!("Cannot create workspace: {e}")))?;
    let canonical = path
        .canonicalize()
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if canonical.parent() != Some(root.as_path())
        || std::fs::symlink_metadata(&path)
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .file_type()
            .is_symlink()
    {
        return Err(ApiError::BadRequest(
            "Managed workspace path changed during creation".into(),
        ));
    }
    Ok(url::Url::from_directory_path(canonical)
        .map_err(|_| ApiError::BadRequest("Invalid workspace path".into()))?
        .to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartWorkArgs {
    reason: String,
}

fn validate_reason(reason: &str) -> Result<(), ApiError> {
    if reason.trim().is_empty()
        || reason.chars().count() > 300
        || reason.chars().any(char::is_control)
    {
        return Err(ApiError::BadRequest(
            "Work reason must contain 1 to 300 printable characters".into(),
        ));
    }
    Ok(())
}

fn remove_empty_workspace(uri: &str) {
    // Only a just-created directory; never recursively delete user data.
    if let Ok(url) = url::Url::parse(uri)
        && let Ok(path) = url.to_file_path()
    {
        let _ = std::fs::remove_dir(path);
    }
}

async fn promote(
    state: &AppState,
    scope: &Scope,
    id: &Id,
    reason: &str,
    turn_id: Option<&Id>,
    cancellation: Option<&CancellationToken>,
) -> Result<Session, ApiError> {
    promote_with_context(state, scope, id, reason, turn_id, cancellation, None).await
}

async fn promote_with_context(
    state: &AppState,
    scope: &Scope,
    id: &Id,
    reason: &str,
    turn_id: Option<&Id>,
    cancellation: Option<&CancellationToken>,
    prepared_context: Option<&mut WorkContext>,
) -> Result<Session, ApiError> {
    validate_reason(reason)?;
    let _guard = state.workspace_transition.lock().await;
    let session = state.store.get_session(id).await?;
    if &session.scope != scope {
        return Err(ApiError::Forbidden);
    }
    if session.status != SessionStatus::Active {
        return Err(ApiError::Conflict("Session is not active".into()));
    }
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return Err(ApiError::Conflict("Turn cancelled".into()));
    }
    if session.mode == SessionMode::Work {
        return Ok(session);
    }
    // Admission is repeated atomically by storage after directory creation.
    if let Some(turn_id) = turn_id {
        let turn = state.store.get_turn(scope, turn_id).await?;
        if turn.session_id != *id
            || !matches!(
                turn.status,
                TurnStatus::PreparingContext | TurnStatus::CallingModel | TurnStatus::RunningTool
            )
        {
            return Err(ApiError::Conflict("Turn is no longer running".into()));
        }
    } else if state.store.list_turns(scope, id).await?.iter().any(|turn| {
        matches!(
            turn.status,
            TurnStatus::Idle
                | TurnStatus::PreparingContext
                | TurnStatus::CallingModel
                | TurnStatus::RunningTool
                | TurnStatus::AwaitingInput
                | TurnStatus::AwaitingApproval
        )
    }) {
        return Err(ApiError::Conflict(
            "Wait for the current response before starting Work".into(),
        ));
    }
    let uri = if let Some(prepared) = prepared_context {
        let (uri, context) = create_prepared_workspace(state, &session, |candidate| async move {
            prepare_work_context(state, &candidate, turn_id).await
        })
        .await?;
        *prepared = context;
        uri
    } else {
        create_managed_workspace(state)?
    };
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        remove_empty_workspace(&uri);
        return Err(ApiError::Conflict("Turn cancelled".into()));
    }
    let session = match state
        .store
        .promote_session_to_work(scope, id, &uri, reason, turn_id)
        .await
    {
        Ok(session) => session,
        Err(error) => {
            remove_empty_workspace(&uri);
            return Err(error.into());
        }
    };
    if session.workspace_uri != uri {
        remove_empty_workspace(&uri);
    }
    if let Err(error) = state.publish(Event {
        id: Id::new("evt"), sequence: 0, timestamp: Utc::now(), scope: scope.clone(), session_id: Some(id.clone()), turn_id: turn_id.cloned(), kind: "session.mode_changed".into(),
        payload: serde_json::json!({"mode":"work", "workspace_uri":session.workspace_uri, "reason":reason}),
    }).await {
        // The mode and reason are already durable. Return the committed result
        // so the model can continue; reconnect restores the same Session.
        tracing::warn!(?error, session_id = %id.0, "Work transition event publication failed");
    }
    Ok(session)
}

pub(super) async fn start_session_work(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<s_code_protocol::StartSessionWork>,
) -> Result<Json<Session>, ApiError> {
    let auth = authorize(&state, &headers)?;
    auth.ensure_scope(&input.scope)?;
    Ok(Json(
        promote(&state, &input.scope, &Id(id), &input.reason, None, None).await?,
    ))
}

pub(super) fn start_work_definition() -> ToolDefinition {
    ToolDefinition {
        name: "start_work".into(), description: "Switch this Chat to Work when the user requests creating/editing files or a project task. Creates a new private directory and keeps conversation history. For ordinary Q&A or inline code examples, answer directly. Cannot select an existing project. Does not grant tool approvals.".into(),
        parameters: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"reason":{"type":"string","minLength":1,"maxLength":300}},"required":["reason"]}),
    }
}

async fn create_prepared_workspace<F, Fut>(
    state: &AppState,
    session: &Session,
    prepare: F,
) -> Result<(String, WorkContext), ApiError>
where
    F: FnOnce(Session) -> Fut,
    Fut: std::future::Future<Output = Result<WorkContext, ApiError>>,
{
    let uri = create_managed_workspace(state)?;
    let mut candidate = session.clone();
    candidate.mode = SessionMode::Work;
    candidate.workspace_uri = uri.clone();
    match prepare(candidate).await {
        Ok(context) => Ok((uri, context)),
        Err(error) => {
            remove_empty_workspace(&uri);
            Err(error)
        }
    }
}

#[derive(Default)]
struct WorkContext {
    tools: Vec<ToolDefinition>,
    system_messages: Vec<ModelMessage>,
}

async fn prepare_work_context(
    state: &AppState,
    session: &Session,
    turn_id: Option<&Id>,
) -> Result<WorkContext, ApiError> {
    let preferences = state
        .store
        .get_session_preferences(&session.scope, &session.id)
        .await?;
    let profile = if preferences.permission_mode == PermissionMode::Plan {
        ToolProfile::Plan
    } else {
        ToolProfile::Default
    };
    let mut tools = tools_for_profile(state, profile);
    let mut system_messages = vec![ModelMessage {
        role: "system".into(),
        content: serde_json::json!(WORK_SYSTEM_PROMPT),
    }];
    let history = state
        .store
        .list_messages(&session.scope, &session.id)
        .await?;
    let active_prompt = history
        .iter()
        .rev()
        .find(|message| message.role == "user" && turn_id.is_none_or(|id| message.turn_id == *id))
        .and_then(|message| message.content.as_str());
    let context = collect_context_items(
        state,
        &session.scope,
        &session.id,
        &session.workspace_uri,
        active_prompt,
    )
    .await?;
    let packed = pack(context, &ContextBudget::default());
    system_messages.push(ModelMessage {
        role: "system".into(),
        content: serde_json::json!({
            "workspace_uri": session.workspace_uri,
            "instructions": "Treat supplied context as scoped evidence; tool permissions still apply.",
            "context": packed.items.iter().map(|item| serde_json::json!({"content":item.content,"source":item.provenance.source_uri})).collect::<Vec<_>>()
        }),
    });
    editing::configure(
        &mut system_messages,
        &mut tools,
        state.editing_profiles.resolve(&session.model),
    );
    Ok(WorkContext {
        tools,
        system_messages,
    })
}

impl DaemonToolExecutor {
    pub(super) async fn start_work(
        &self,
        arguments: serde_json::Value,
        cancellation: &CancellationToken,
    ) -> AgentToolResult {
        let result = async {
            let args: StartWorkArgs = serde_json::from_value(arguments).map_err(|error| ApiError::BadRequest(error.to_string()))?;
            let mut context = WorkContext::default();
            let session = promote_with_context(&self.state, &self.scope, &self.session_id, &args.reason, Some(&self.turn_id), Some(cancellation), Some(&mut context)).await?;
            Ok::<_, ApiError>(AgentToolResult::ContextChanged {
                retired_system_content: serde_json::json!(CHAT_SYSTEM_PROMPT),
                value: serde_json::json!({"mode":"work", "workspace_uri":session.workspace_uri, "reason":args.reason}),
                tools: context.tools,
                system_messages: context.system_messages,
            })
        }.await;
        result.unwrap_or_else(|error| AgentToolResult::Failed {
            error: format!("Cannot start Work: {error:?}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s_code_model_gateway::{GatewayError, ModelStream};

    async fn fixture() -> (tempfile::TempDir, AppState, Session) {
        let temp = tempfile::tempdir().unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("actor".into()),
            goal_id: None,
            task_id: None,
        };
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                mode: SessionMode::Chat,
                scope,
                workspace_uri: String::new(),
                title: "Chat".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let mut state = AppState::new("secret", store, 0);
        state.managed_workspaces = Some(Arc::new(temp.path().join("workspaces")));
        (temp, state, session)
    }

    struct ChatProvider {
        work: bool,
        requests: StdMutex<Vec<ModelRequest>>,
    }
    #[async_trait::async_trait]
    impl ModelProvider for ChatProvider {
        async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            requests.push(request);
            let call = if self.work && index == 0 {
                Some((
                    "start_work",
                    serde_json::json!({"reason":"Create the requested document"}),
                ))
            } else if self.work && index == 1 {
                Some((
                    "apply_patch",
                    serde_json::json!({"files":[{"path":"hello.txt","expected_revision":null,"content":"hello from Work\n"}]}),
                ))
            } else {
                None
            };
            let mut events = Vec::new();
            if let Some((tool, ref args)) = call {
                events.push(Ok(ModelEvent::ToolCallDelta {
                    index: 0,
                    id: Some(format!("call_{index}")),
                    name: Some(tool.into()),
                    arguments_delta: args.to_string(),
                    provider_metadata: None,
                }));
            } else {
                events.push(Ok(ModelEvent::TextDelta {
                    text: "Done".into(),
                }));
            }
            events.push(Ok(ModelEvent::Completed {
                finish_reason: Some(if call.is_some() { "tool_calls" } else { "stop" }.into()),
            }));
            Ok(Box::pin(futures_util::stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn chat_qa_has_no_workspace_context_or_local_tools() {
        let (_temp, state, session) = fixture().await;
        let provider = Arc::new(ChatProvider {
            work: false,
            requests: StdMutex::new(Vec::new()),
        });
        let turn = state
            .store
            .create_turn(&session.scope, &session.id)
            .await
            .unwrap();
        state
            .store
            .append_turn_message(
                &session.scope,
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("Explain recursion"),
            )
            .await
            .unwrap();
        run_turn_with_step_inputs(
            state.clone(),
            provider.clone(),
            turn.clone(),
            CancellationToken::new(),
            ToolProfile::Default,
            None,
            false,
        )
        .await
        .unwrap();
        let requests = provider.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["start_work"]
        );
        assert!(
            requests[0].messages[0]
                .content
                .as_str()
                .unwrap()
                .contains("Chat mode")
        );
        assert!(!state.managed_workspaces.as_ref().unwrap().exists());
        assert_eq!(
            state.store.get_session(&session.id).await.unwrap().mode,
            SessionMode::Chat
        );
        assert_eq!(
            state
                .store
                .get_turn(&session.scope, &turn.id)
                .await
                .unwrap()
                .status,
            TurnStatus::Completed
        );
        assert!(
            collect_context_items(&state, &session.scope, &session.id, "", Some("skill"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn chat_promotes_and_edits_without_git_in_same_turn() {
        let (_temp, state, session) = fixture().await;
        state
            .store
            .update_session_preferences(
                &session.id,
                UpdateSessionPreferences {
                    scope: session.scope.clone(),
                    permission_mode: Some(PermissionMode::AcceptEdits),
                    assistant_alias: None,
                },
            )
            .await
            .unwrap();
        let provider = Arc::new(ChatProvider {
            work: true,
            requests: StdMutex::new(Vec::new()),
        });
        for (id, term) in [("relevant", "hello.txt"), ("unrelated", "bananas")] {
            state
                .store
                .install_skill(
                    &session.scope,
                    &s_code_protocol::SkillSpec {
                        id: id.into(),
                        name: id.into(),
                        description: id.into(),
                        source_uri: format!("file:///skills/{id}"),
                        activation_terms: vec![term.into()],
                        mcp_dependencies: Vec::new(),
                        auto_match: true,
                    },
                    &format!("{id}-skill-instructions"),
                    &format!(
                        "{:x}",
                        Sha256::digest(format!("{id}-skill-instructions").as_bytes())
                    ),
                    "fixture-permissions",
                )
                .await
                .unwrap();
        }
        for index in 0..70 {
            state
                .store
                .append_message(
                    &session.scope,
                    &session.id,
                    if index % 2 == 0 { "user" } else { "assistant" },
                    serde_json::json!(format!("old-requirement-{index} {}", "x".repeat(4_000))),
                )
                .await
                .unwrap();
        }
        let turn = state
            .store
            .create_turn(&session.scope, &session.id)
            .await
            .unwrap();
        state
            .store
            .append_turn_message(
                &session.scope,
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("Create hello.txt"),
            )
            .await
            .unwrap();
        run_turn_with_step_inputs(
            state.clone(),
            provider.clone(),
            turn.clone(),
            CancellationToken::new(),
            ToolProfile::Default,
            None,
            false,
        )
        .await
        .unwrap();
        let updated = state.store.get_session(&session.id).await.unwrap();
        assert_eq!(updated.mode, SessionMode::Work);
        assert_eq!(
            updated.work_reason.as_deref(),
            Some("Create the requested document")
        );
        let path = url::Url::parse(&updated.workspace_uri)
            .unwrap()
            .to_file_path()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("hello.txt")).unwrap(),
            "hello from Work\n"
        );
        assert!(!path.join(".git").exists());
        assert_eq!(
            state
                .store
                .get_turn(&session.scope, &turn.id)
                .await
                .unwrap()
                .status,
            TurnStatus::Completed
        );
        let events = state
            .store
            .list_session_events(&session.scope, &session.id, 100)
            .await
            .unwrap();
        assert!(
            events.iter().any(
                |event| event.kind == "tool.completed" && event.payload["tool"] == "start_work"
            )
        );
        let requests = provider.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 3);
        let work_context = serde_json::to_string(&requests[1].messages).unwrap();
        assert!(work_context.contains("relevant-skill-instructions"));
        assert!(!work_context.contains("unrelated-skill-instructions"));
        for request in &requests[..2] {
            let encoded = serde_json::to_string(&request.messages).unwrap();
            assert!(encoded.contains("internal://conversation-compaction"));
            assert!(encoded.contains("old-requirement-0"));
        }
        assert!(
            requests[1]
                .tools
                .iter()
                .any(|tool| tool.name == "apply_patch")
        );
        assert!(
            !requests[1]
                .tools
                .iter()
                .any(|tool| tool.name == "start_work")
        );
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == "user"
                    && message.content == serde_json::json!("Create hello.txt"))
        );
        assert!(!requests[1].messages.iter().any(|message| {
            message.role == "system"
                && message
                    .content
                    .as_str()
                    .is_some_and(|text| text.contains("You are in Chat mode"))
        }));
    }

    #[tokio::test]
    async fn chat_denies_direct_and_forged_tools_before_hooks() {
        let (_temp, state, session) = fixture().await;
        let turn = state
            .store
            .create_turn(&session.scope, &session.id)
            .await
            .unwrap();
        let executor = DaemonToolExecutor {
            state: state.clone(),
            session_id: session.id.clone(),
            turn_id: turn.id,
            scope: session.scope.clone(),
        };
        for tool in [
            "read_file",
            "run_command",
            "tool_search",
            "mcp.fake.read",
            "create_goal",
        ] {
            assert!(matches!(
                executor
                    .execute(
                        "forged",
                        tool,
                        serde_json::json!({}),
                        &CancellationToken::new()
                    )
                    .await,
                AgentToolResult::Failed { .. }
            ));
        }
        assert!(
            state
                .execution
                .submit(
                    &session.id,
                    SubmitToolCall {
                        scope: session.scope.clone(),
                        tool: "read_file".into(),
                        arguments: serde_json::json!({"path":"secret"})
                    }
                )
                .await
                .is_err()
        );
        assert!(!state.managed_workspaces.as_ref().unwrap().exists());
    }

    #[tokio::test]
    async fn promotion_is_scoped_idempotent_and_cancel_safe() {
        let (_temp, state, session) = fixture().await;
        let mut foreign = session.scope.clone();
        foreign.actor_id = Id("other".into());
        assert!(
            promote(&state, &foreign, &session.id, "work", None, None)
                .await
                .is_err()
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(
            promote(
                &state,
                &session.scope,
                &session.id,
                "work",
                None,
                Some(&cancellation)
            )
            .await
            .is_err()
        );
        assert!(!state.managed_workspaces.as_ref().unwrap().exists());
        let (first, second) = tokio::join!(
            promote(&state, &session.scope, &session.id, "first", None, None),
            promote(&state, &session.scope, &session.id, "second", None, None)
        );
        assert_eq!(first.unwrap().workspace_uri, second.unwrap().workspace_uri);
        assert_eq!(
            std::fs::read_dir(state.managed_workspaces.as_ref().unwrap().as_ref())
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cancelled_turn_cannot_promote_and_chat_fork_stays_chat() {
        let (_temp, state, session) = fixture().await;
        let turn = state
            .store
            .create_turn(&session.scope, &session.id)
            .await
            .unwrap();
        state
            .store
            .update_turn(&session.scope, &turn.id, TurnStatus::Cancelled, None, None)
            .await
            .unwrap();
        assert!(
            promote(
                &state,
                &session.scope,
                &session.id,
                "work",
                Some(&turn.id),
                None
            )
            .await
            .is_err()
        );
        assert!(!state.managed_workspaces.as_ref().unwrap().exists());
        let fork = create_session_branch(
            &state,
            &session.scope,
            &session,
            Vec::new(),
            None,
            "Fork".into(),
            "test",
        )
        .await
        .unwrap();
        assert_eq!(fork.mode, SessionMode::Chat);
        assert!(fork.workspace_uri.is_empty());
    }

    #[tokio::test]
    async fn chat_settings_allow_no_workspace_and_legacy_clients_remain_work() {
        let (_temp, state, _) = fixture().await;
        let settings = s_code_protocol::DaemonSettings::default();
        assert!(settings.workspace_uri.is_empty());
        state.store.put_settings(&settings).await.unwrap();
        assert!(
            state
                .store
                .get_settings()
                .await
                .unwrap()
                .workspace_uri
                .is_empty()
        );
        let mut invalid = settings.clone();
        invalid.workspace_uri = "not a URI".into();
        assert!(state.store.put_settings(&invalid).await.is_err());
        let old:CreateSession=serde_json::from_value(serde_json::json!({"scope":{"organization_id":"org","team_id":"team","actor_id":"actor"},"workspace_uri":"file:///workspace","title":"Legacy","model":"mock"})).unwrap();
        assert_eq!(old.mode, SessionMode::Work);
    }

    #[tokio::test]
    async fn failed_context_preparation_leaves_chat_and_cleans_empty_directory() {
        let (_temp, state, session) = fixture().await;
        let failed = create_prepared_workspace(&state, &session, |_| async {
            Err(ApiError::Unavailable("context fixture failed".into()))
        })
        .await;
        assert!(failed.is_err());
        assert_eq!(
            state.store.get_session(&session.id).await.unwrap().mode,
            SessionMode::Chat
        );
        assert_eq!(
            std::fs::read_dir(state.managed_workspaces.as_ref().unwrap().as_ref())
                .unwrap()
                .count(),
            0
        );
        let work = promote(&state, &session.scope, &session.id, "Retry", None, None)
            .await
            .unwrap();
        assert_eq!(work.mode, SessionMode::Work);
    }

    #[tokio::test]
    async fn chat_goal_api_rejects_auto_continue_without_creating_a_goal() {
        let (_temp, state, session) = fixture().await;
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer secret".parse().unwrap());
        let result = set_session_goal(
            State(state.clone()),
            headers,
            Path(session.id.0.clone()),
            Json(SetSessionGoal {
                scope: session.scope.clone(),
                objective: "Explain recursion".into(),
                auto_continue: true,
                token_budget: None,
            }),
        )
        .await;
        assert!(matches!(result, Err(ApiError::Conflict(_))));
        assert!(
            state
                .store
                .get_session_goal(&session.scope, &session.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!state.managed_workspaces.as_ref().unwrap().exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_workspace_root_is_rejected() {
        let (temp, mut state, _) = fixture().await;
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        state.managed_workspaces = Some(Arc::new(link));
        assert!(create_managed_workspace(&state).is_err());
        assert_eq!(std::fs::read_dir(target).unwrap().count(), 0);
    }
}
