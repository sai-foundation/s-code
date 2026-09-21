//! User-controlled, durable file rules. This module never calls a model to interpret a rule.
use super::*;
use s_code_storage::{FileProtectionPolicy, FileProtectionRule};
use std::path::{Component, Path as FsPath, PathBuf};

#[derive(Deserialize)]
pub(super) struct QueryScope {
    organization_id: String,
    team_id: String,
    actor_id: String,
    expected_revision: Option<u64>,
}
impl QueryScope {
    fn scope(&self) -> Scope {
        Scope {
            organization_id: Id(self.organization_id.clone()),
            team_id: Id(self.team_id.clone()),
            actor_id: Id(self.actor_id.clone()),
            goal_id: None,
            task_id: None,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AddRule {
    scope: Scope,
    path: String,
    expected_revision: u64,
}

pub(super) fn same_owner(left: &Scope, right: &Scope) -> bool {
    left.organization_id == right.organization_id
        && left.team_id == right.team_id
        && left.actor_id == right.actor_id
}
async fn session(state: &AppState, scope: &Scope, id: &Id) -> Result<Session, ApiError> {
    let session = state.store.get_session(id).await?;
    if !same_owner(&session.scope, scope) {
        return Err(ApiError::Forbidden);
    }
    Ok(session)
}
pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<QueryScope>,
) -> Result<Json<FileProtectionPolicy>, ApiError> {
    let scope = query.scope();
    authorize(&state, &headers)?.ensure_scope(&scope)?;
    session(&state, &scope, &Id(id)).await?;
    Ok(Json(state.store.file_protection(&scope).await?))
}
pub(super) async fn add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AddRule>,
) -> Result<Json<FileProtectionPolicy>, ApiError> {
    authorize(&state, &headers)?.ensure_scope(&input.scope)?;
    let session = session(&state, &input.scope, &Id(id)).await?;
    Ok(Json(
        add_path(
            &state,
            &input.scope,
            &session,
            &input.path,
            input.expected_revision,
        )
        .await?,
    ))
}
pub(super) async fn remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, rule_id)): Path<(String, String)>,
    Query(query): Query<QueryScope>,
) -> Result<Json<FileProtectionPolicy>, ApiError> {
    let scope = query.scope();
    authorize(&state, &headers)?.ensure_scope(&scope)?;
    let session = session(&state, &scope, &Id(id)).await?;
    let expected = query
        .expected_revision
        .ok_or_else(|| ApiError::BadRequest("expected_revision is required".into()))?;
    let mut policy = state.store.file_protection(&scope).await?;
    if expected != policy.revision {
        return Err(ApiError::Conflict(
            "Protection changed. Refresh and try again.".into(),
        ));
    }
    let before = policy.rules.len();
    policy.rules.retain(|rule| rule.id.0 != rule_id);
    if before == policy.rules.len() {
        return Err(ApiError::NotFound);
    }
    Ok(Json(
        activate(&state, &scope, &session.id, expected, policy.rules).await?,
    ))
}
async fn add_path(
    state: &AppState,
    scope: &Scope,
    session: &Session,
    path: &str,
    expected: u64,
) -> Result<FileProtectionPolicy, ApiError> {
    let mut policy = state.store.file_protection(scope).await?;
    if policy.revision != expected {
        return Err(ApiError::Conflict(
            "Protection changed. Refresh and try again.".into(),
        ));
    }
    let rule = rule_for_path(&session.workspace_uri, path)?;
    if policy
        .rules
        .iter()
        .any(|existing| existing.path == rule.path)
    {
        return Ok(policy);
    }
    policy.rules.push(rule);
    activate(state, scope, &session.id, expected, policy.rules).await
}

fn normalize(path: &FsPath) -> Result<PathBuf, ApiError> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(ApiError::BadRequest("Path escapes its root".into()));
                }
            }
            part => result.push(part.as_os_str()),
        }
    }
    if !result.is_absolute() {
        return Err(ApiError::BadRequest(
            "Choose an absolute path, or a file relative to the working directory".into(),
        ));
    }
    Ok(result)
}
fn rule_for_path(workspace_uri: &str, input: &str) -> Result<FileProtectionRule, ApiError> {
    let value = input.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(
            "Enter one file or directory path (up to 4096 characters)".into(),
        ));
    }
    let mut path = if value.starts_with("file:") {
        let uri = url::Url::parse(value)
            .map_err(|_| ApiError::BadRequest("Invalid local file URI".into()))?;
        if uri.query().is_some() || uri.fragment().is_some() || !uri.username().is_empty() {
            return Err(ApiError::BadRequest(
                "Use a local file path without URL options".into(),
            ));
        }
        uri.to_file_path()
            .map_err(|_| ApiError::BadRequest("Use a local file path".into()))?
    } else {
        if value.contains("://") || value.starts_with('~') {
            return Err(ApiError::BadRequest(
                "Use an absolute local path instead of a URL or ~".into(),
            ));
        }
        PathBuf::from(value)
    };
    if !path.is_absolute() {
        let root = url::Url::parse(workspace_uri)
            .ok()
            .and_then(|uri| uri.to_file_path().ok())
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Chat has no working directory. Enter an absolute file path.".into(),
                )
            })?;
        path = root.join(path);
    }
    let path = normalize(&path)?;
    // No file contents are opened. Keep the lexical path too, so replacement at
    // the same path stays protected. Resolve the nearest existing parent for a
    // not-yet-created file beneath a symlinked directory.
    let mut ancestor = path.clone();
    let mut suffix = Vec::new();
    let canonical = loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(mut canonical) => {
                for part in suffix.iter().rev() {
                    canonical.push(part);
                }
                break canonical;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let part = ancestor
                    .file_name()
                    .ok_or_else(|| ApiError::BadRequest("Cannot resolve path".into()))?
                    .to_owned();
                suffix.push(part);
                ancestor.pop();
            }
            Err(_) => {
                return Err(ApiError::BadRequest(
                    "Cannot resolve this path safely".into(),
                ));
            }
        }
    };
    let metadata = std::fs::metadata(&path).ok();
    let kind = if metadata.as_ref().is_some_and(|m| m.is_dir()) {
        "directory"
    } else {
        "file"
    };
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.as_ref().map(|m| m.dev()),
            metadata.as_ref().map(|m| m.ino()),
        )
    };
    #[cfg(not(unix))]
    let (device, inode) = (None, None);
    Ok(FileProtectionRule {
        id: Id::new("protection"),
        path: path
            .to_str()
            .ok_or_else(|| ApiError::BadRequest("Path must be UTF-8".into()))?
            .into(),
        canonical_path: Some(
            canonical
                .to_str()
                .ok_or_else(|| ApiError::BadRequest("Path must be UTF-8".into()))?
                .into(),
        ),
        device,
        inode,
        kind: kind.into(),
        created_at: Utc::now(),
    })
}

async fn cancel_running(state: &AppState, scope: &Scope) {
    // Snapshot before awaiting storage: never hold the registry lock over I/O.
    let entries = {
        let registry = state.runtime_scopes.inner.lock().await;
        registry
            .sessions
            .iter()
            .map(|(id, runtime)| {
                (
                    id.clone(),
                    runtime
                        .turns
                        .values()
                        .map(|turn| turn.cancellation.clone())
                        .collect::<Vec<_>>(),
                    runtime
                        .background_terminals
                        .values()
                        .map(|terminal| terminal.commands.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    for (id, turns, terminals) in entries {
        if state
            .store
            .get_session(&id)
            .await
            .is_ok_and(|session| same_owner(&session.scope, scope))
        {
            for turn in turns {
                turn.cancel();
            }
            for terminal in terminals {
                let _ = terminal.send(BackgroundTerminalCommand::Stop);
            }
        }
    }
}
async fn activate(
    state: &AppState,
    scope: &Scope,
    session_id: &Id,
    revision: u64,
    rules: Vec<FileProtectionRule>,
) -> Result<FileProtectionPolicy, ApiError> {
    // Dispatches and host operations hold a shared gate. While the exclusive
    // replacement waits, cancel old work and drain terminals. Never acknowledge
    // protection until the durable policy and context barrier are committed.
    cancel_running(state, scope).await;
    let replace = state.store.replace_file_protection(scope, revision, rules);
    tokio::pin!(replace);
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    let policy = loop {
        tokio::select! {
            result = &mut replace => break result.map_err(|error| match error {
                StorageError::InvalidState(message) => ApiError::Conflict(message), other => other.into(),
            })?,
            _ = interval.tick() => cancel_running(state, scope).await,
        }
    };
    state.publish(Event { id: Id::new("evt"), sequence: 0, timestamp: Utc::now(), scope: scope.clone(), session_id: Some(session_id.clone()), turn_id: None, kind: "privacy.protection.updated".into(), payload: serde_json::json!({"revision":policy.revision,"rule_count":policy.rules.len(),"context_reset":true}) }).await?;
    Ok(policy)
}

pub(super) async fn check_turn(
    state: &AppState,
    turn: &Turn,
) -> Result<FileProtectionPolicy, ApiError> {
    let policy = state.store.file_protection(&turn.scope).await?;
    if policy
        .changed_at
        .is_some_and(|cutoff| turn.started_at <= cutoff)
    {
        return Err(ApiError::Conflict(
            "File protection changed. Send a new message to start with fresh context.".into(),
        ));
    }
    Ok(policy)
}
pub(super) async fn fresh_history(
    state: &AppState,
    scope: &Scope,
    session_id: &Id,
    history: &mut Vec<Message>,
) -> Result<(), ApiError> {
    let policy = state.store.file_protection(scope).await?;
    if let Some(cutoff) = policy.changed_at {
        let safe_turns: HashSet<Id> = state
            .store
            .list_turns(scope, session_id)
            .await?
            .into_iter()
            .filter(|turn| turn.started_at > cutoff)
            .map(|turn| turn.id)
            .collect();
        // Use originating turn time: a late pre-protection response must not
        // become fresh context merely because it was persisted after activation.
        history.retain(|message| safe_turns.contains(&message.turn_id));
    }
    Ok(())
}
pub(super) async fn require_host_access(
    state: &AppState,
    scope: &Scope,
) -> Result<tokio::sync::OwnedRwLockReadGuard<()>, ApiError> {
    let guard = state.store.protection_gate(scope).read_owned().await;
    if !state.store.file_protection(scope).await?.rules.is_empty() {
        return Err(ApiError::Conflict("Hard file protection blocks shell, Git, MCP and other host execution. Manage protection in Privacy.".into()));
    }
    Ok(guard)
}

pub(super) fn chat_path(content: &serde_json::Value) -> Option<&str> {
    let text = content.as_str()?.trim();
    for prefix in ["/protect", "protect file", "保护文件", "保护目录"] {
        if let Some(path) = text.strip_prefix(prefix)
            && (path.is_empty()
                || path.starts_with(char::is_whitespace)
                || prefix.starts_with('保'))
        {
            return Some(path.trim().trim_matches('`').trim_matches('"'));
        }
    }
    None
}
pub(super) async fn local_chat(
    state: &AppState,
    scope: &Scope,
    session_id: &Id,
    content: &serde_json::Value,
) -> Result<Option<Turn>, ApiError> {
    let Some(path) = chat_path(content) else {
        return Ok(None);
    };
    let session = session(state, scope, session_id).await?;
    let policy = state.store.file_protection(scope).await?;
    let policy = add_path(state, scope, &session, path, policy.revision).await?;
    let turn = state.store.create_turn(scope, session_id).await?;
    let user = state
        .store
        .append_turn_message(scope, session_id, &turn.id, "user", content.clone())
        .await?;
    state
        .publish(Event {
            id: Id::new("evt"),
            sequence: 0,
            timestamp: Utc::now(),
            scope: scope.clone(),
            session_id: Some(session_id.clone()),
            turn_id: Some(turn.id.clone()),
            kind: "turn.created".into(),
            payload: serde_json::json!({"status":turn.status,"item_id":user.id,"local":true}),
        })
        .await?;
    let assistant_item_id = Id::new("item");
    let confirmation = format!(
        "Protected locally. {} file or directory rule(s) are active for this account. Older model context has been isolated. Previously sent content cannot be recalled. Manage rules in Privacy.",
        policy.rules.len()
    );
    state
        .store
        .append_turn_message_with_id(
            scope,
            session_id,
            &turn.id,
            assistant_item_id.clone(),
            "assistant",
            serde_json::json!(confirmation),
        )
        .await?;
    // Reuse the transcript text event, explicitly marked local. This is not a
    // provider dispatch and creates no outbound privacy ledger entry.
    state.publish(Event { id:Id::new("evt"),sequence:0,timestamp:Utc::now(),scope:scope.clone(),session_id:Some(session_id.clone()),turn_id:Some(turn.id.clone()),kind:"model.delta".into(),payload:serde_json::json!({"item_id":assistant_item_id,"text":confirmation,"byte_offset":0,"local":true}) }).await?;
    state
        .store
        .update_turn(scope, &turn.id, TurnStatus::PreparingContext, None, None)
        .await?;
    state
        .store
        .update_turn(scope, &turn.id, TurnStatus::CallingModel, None, None)
        .await?;
    state
        .store
        .update_turn(scope, &turn.id, TurnStatus::Completed, None, None)
        .await?;
    state
        .publish(Event {
            id: Id::new("evt"),
            sequence: 0,
            timestamp: Utc::now(),
            scope: scope.clone(),
            session_id: Some(session_id.clone()),
            turn_id: Some(turn.id.clone()),
            kind: "turn.completed".into(),
            payload: serde_json::json!({"item_id":assistant_item_id,"status":"completed","local":true,"model_calls":0,"tool_calls":0}),
        })
        .await?;
    Ok(Some(state.store.get_turn(scope, &turn.id).await?))
}

pub(super) fn safe_tool(tool: &str) -> bool {
    s_code_execution::safe_protected_tool(tool)
        || matches!(
            tool,
            "start_work"
                | "execute"
                | "update_plan"
                | "request_user_input"
                | "report_review_findings"
        )
}
pub(super) async fn check_agent_tool(
    state: &AppState,
    scope: &Scope,
    turn_id: &Id,
    tool: &str,
) -> Result<(), String> {
    let turn = state
        .store
        .get_turn(scope, turn_id)
        .await
        .map_err(|error| error.to_string())?;
    let policy = check_turn(state, &turn)
        .await
        .map_err(|error| format!("{error:?}"))?;
    if !policy.rules.is_empty() && !safe_tool(tool) {
        return Err("This tool is unavailable while hard file protection is active. Use guarded file tools; only the user can change protection in Privacy.".into());
    }
    // Old goals can carry file excerpts even after the last rule is removed.
    if matches!(tool, "get_goal" | "update_goal" | "create_goal") {
        let goal = state
            .store
            .get_session_goal(scope, &turn.session_id)
            .await
            .map_err(|error| error.to_string())?;
        if goal.is_some_and(|goal| {
            policy
                .changed_at
                .is_some_and(|cutoff| goal.created_at <= cutoff)
        }) {
            return Err(
                "This goal predates a privacy reset. Create a fresh goal in the UI.".into(),
            );
        }
    }
    Ok(())
}

/// Preserve original content provenance while a retry is prepared asynchronously.
pub(super) struct TurnContent {
    pub value: serde_json::Value,
    pub origin_started_at: Option<chrono::DateTime<Utc>>,
}
impl From<serde_json::Value> for TurnContent {
    fn from(value: serde_json::Value) -> Self {
        Self {
            value,
            origin_started_at: None,
        }
    }
}

pub(super) async fn check_user_content(
    state: &AppState,
    scope: &Scope,
    content: &serde_json::Value,
) -> Result<(), ApiError> {
    if !state.store.file_protection(scope).await?.rules.is_empty() && !content.is_string() {
        return Err(ApiError::Conflict("Hard file protection blocks attachments and structured user content; send a text message.".into()));
    }
    Ok(())
}
