//! IM channels are clients of the existing execution/approval boundary.
//! Persist receipt before dispatch: uncertain deliveries are never auto-replayed.
use super::*;
use s_code_connector_sdk::im::{
    ImButton, ImChannel, ImError, ImUpdate, ImUpdateKind, TelegramChannel, truncate_text,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[cfg(test)]
mod tests;

const CHANNEL: &str = "telegram";
const MAX_SESSIONS: usize = 20;
const MAX_OUTBOX: usize = 32;
const PAIR_SECONDS: i64 = 600;

#[derive(Default, Serialize, Deserialize)]
struct ChannelState {
    token: String,
    username: String,
    generation: String,
    offset: i64,
    #[serde(default)]
    interrupted_update: bool,
    pairing: Option<Pairing>,
    binding: Option<Binding>,
    sessions: Vec<Id>,
    outbox: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Pairing {
    code_hash: String,
    expires: i64,
    pending_user: Option<i64>,
    pending_chat: Option<i64>,
}
#[derive(Serialize, Deserialize)]
struct Binding {
    user_id: i64,
    chat_id: i64,
    selected: Option<Id>,
    active: Option<Delivery>,
}
#[derive(Serialize, Deserialize)]
struct Delivery {
    session_id: Id,
    turn_id: Option<Id>,
    message_id: Option<i64>,
    rendered: String,
    approval: Option<ButtonGrant>,
}
#[derive(Serialize, Deserialize)]
struct ButtonGrant {
    #[serde(default)]
    can_approve: bool,
    nonce: String,
    approval_id: Id,
    digest: String,
    expires: i64,
}
struct Loaded {
    revision: Option<i64>,
    data: ChannelState,
}
impl Loaded {
    async fn load(state: &AppState, scope: &Scope) -> Result<Self, ApiError> {
        match state.store.get_im_state(scope, CHANNEL).await? {
            Some(record) => Ok(Self {
                revision: Some(record.revision),
                data: serde_json::from_value(record.value)
                    .map_err(|_| ApiError::Internal("invalid IM state".into()))?,
            }),
            None => Ok(Self {
                revision: None,
                data: ChannelState::default(),
            }),
        }
    }
    async fn save(&mut self, state: &AppState, scope: &Scope) -> Result<(), ApiError> {
        let value = serde_json::to_value(&self.data)
            .map_err(|_| ApiError::Internal("cannot encode IM state".into()))?;
        self.revision = Some(
            state
                .store
                .put_im_state(scope, CHANNEL, self.revision, &value)
                .await?
                .revision,
        );
        Ok(())
    }
    fn queue(&mut self, text: impl Into<String>) {
        if self.data.outbox.len() < MAX_OUTBOX {
            self.data.outbox.push(truncate_text(&text.into(), 4000));
        }
    }
}
fn secret() -> Result<String, ApiError> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|_| ApiError::Internal("cannot generate pairing secret".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
async fn current_scope(state: &AppState) -> Result<Scope, ApiError> {
    let settings = state.store.get_settings().await?;
    Ok(Scope {
        organization_id: settings.organization_id,
        team_id: settings.team_id,
        actor_id: settings.actor_id,
        goal_id: None,
        task_id: None,
    })
}
fn same_actor(a: &Scope, b: &Scope) -> bool {
    a.organization_id == b.organization_id && a.team_id == b.team_id && a.actor_id == b.actor_id
}
async fn ensure_local(
    state: &AppState,
    headers: &HeaderMap,
    scope: &Scope,
) -> Result<(), ApiError> {
    if !matches!(authorize(state, headers)?, AuthContext::Development) {
        return Err(ApiError::Forbidden);
    }
    if !same_actor(scope, &current_scope(state).await?) {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}
fn internal_headers(state: &AppState) -> Result<HeaderMap, ApiError> {
    let InteractiveAuth::DevelopmentToken(token) = &state.interactive_auth else {
        return Err(ApiError::Forbidden);
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .map_err(|_| ApiError::Unauthorized)?,
    );
    Ok(headers)
}
fn public_status(data: &ChannelState) -> Value {
    json!({"channel":CHANNEL,"configured":!data.token.is_empty(),"bot_username":data.username,
        "paired_user":data.binding.as_ref().map(|b|b.user_id),
        "pending_user":data.pairing.as_ref().filter(|p|p.expires > Utc::now().timestamp()).and_then(|p|p.pending_user),
        "allowed_sessions":data.sessions,
        "selected_session":data.binding.as_ref().and_then(|b|b.selected.as_ref()),
        "active_turn":data.binding.as_ref().and_then(|b|b.active.as_ref()).and_then(|a|a.turn_id.as_ref())})
}
#[derive(Deserialize)]
pub(super) struct ManageRequest {
    scope: Scope,
    #[serde(flatten)]
    action: ManageAction,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ManageAction {
    Connect { token: String },
    Pair {},
    Approve { user_id: i64 },
    Allow { session_id: Id },
    Disallow { session_id: Id },
    Revoke {},
    Disconnect {},
}
pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MessageQuery>,
) -> Result<Json<Value>, ApiError> {
    let scope = Scope {
        organization_id: Id(query.organization_id),
        team_id: Id(query.team_id),
        actor_id: Id(query.actor_id),
        goal_id: None,
        task_id: None,
    };
    ensure_local(&state, &headers, &scope).await?;
    Ok(Json(public_status(
        &Loaded::load(&state, &scope).await?.data,
    )))
}
pub(super) async fn manage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ManageRequest>,
) -> Result<Json<Value>, ApiError> {
    ensure_local(&state, &headers, &input.scope).await?;
    let _guard = state.im_lock.lock().await;
    ensure_local(&state, &headers, &input.scope).await?;
    let mut loaded = Loaded::load(&state, &input.scope).await?;
    let mut pairing_link = None;
    match input.action {
        ManageAction::Connect { token } => {
            if !matches!(
                state.storage_protection.as_ref(),
                "managed_encrypted" | "explicit_encrypted"
            ) {
                return Err(ApiError::BadRequest(
                    "Telegram requires encrypted local storage; restart with a managed storage key"
                        .into(),
                ));
            }
            if !loaded.data.token.is_empty() {
                return Err(ApiError::Conflict(
                    "Disconnect the existing bot before replacing it".into(),
                ));
            }
            let channel = TelegramChannel::new(token.clone()).map_err(transport_error)?;
            let bot = channel
                .get_me()
                .await
                .map_err(|e| runtime_transport_error(&state, e))?;
            state.im_backoff_until.store(0, Ordering::SeqCst);
            loaded.data = ChannelState {
                token,
                username: bot.username,
                generation: secret()?,
                ..Default::default()
            };
            pairing_link = Some(issue_pair(&mut loaded)?);
        }
        ManageAction::Pair {} => {
            if loaded.data.token.is_empty() {
                return Err(ApiError::BadRequest("Connect a Telegram bot first".into()));
            }
            if loaded.data.binding.is_some() {
                return Err(ApiError::Conflict(
                    "Revoke the current phone before pairing another".into(),
                ));
            }
            pairing_link = Some(issue_pair(&mut loaded)?);
        }
        ManageAction::Approve { user_id } => {
            let pending = loaded
                .data
                .pairing
                .as_ref()
                .filter(|p| p.expires > Utc::now().timestamp() && p.pending_user == Some(user_id))
                .ok_or_else(|| {
                    ApiError::BadRequest("No matching unexpired pairing request".into())
                })?;
            let chat_id = pending.pending_chat.ok_or(ApiError::Forbidden)?;
            loaded.data.binding = Some(Binding {
                user_id,
                chat_id,
                selected: loaded.data.sessions.first().cloned(),
                active: None,
            });
            loaded.data.pairing = None;
            loaded.queue("Connected to S-Code. Use /sessions to choose an authorized session, then send a task. /help lists commands.");
        }
        ManageAction::Allow { session_id } => {
            let session = state.store.get_session(&session_id).await?;
            if !same_actor(&session.scope, &input.scope) || session.status != SessionStatus::Active
            {
                return Err(ApiError::Forbidden);
            }
            if loaded.data.sessions.len() >= MAX_SESSIONS {
                return Err(ApiError::BadRequest(
                    "At most 20 sessions may be authorized".into(),
                ));
            }
            if !loaded.data.sessions.contains(&session_id) {
                loaded.data.sessions.push(session_id.clone());
            }
            if let Some(binding) = &mut loaded.data.binding {
                binding.selected.get_or_insert(session_id);
            }
        }
        ManageAction::Disallow { session_id } => {
            loaded.data.sessions.retain(|id| id != &session_id);
            if let Some(binding) = &mut loaded.data.binding {
                if binding.selected.as_ref() == Some(&session_id) {
                    binding.selected = None;
                }
                if binding
                    .active
                    .as_ref()
                    .is_some_and(|d| d.session_id == session_id)
                {
                    binding.active = None;
                }
            }
            loaded.data.outbox.clear();
        }
        ManageAction::Revoke {} => {
            loaded.data.binding = None;
            loaded.data.pairing = None;
            loaded.data.outbox.clear();
            loaded.data.generation = secret()?;
        }
        ManageAction::Disconnect {} => {
            loaded.data = ChannelState::default();
        }
    }
    loaded.save(&state, &input.scope).await?;
    let mut status = public_status(&loaded.data);
    if let Some(link) = pairing_link {
        status["pairing_link"] = json!(link);
        status["pairing_expires_in_seconds"] = json!(PAIR_SECONDS);
    }
    Ok(Json(status))
}
fn issue_pair(loaded: &mut Loaded) -> Result<String, ApiError> {
    let code = secret()?;
    loaded.data.pairing = Some(Pairing {
        code_hash: digest(&code),
        expires: Utc::now().timestamp() + PAIR_SECONDS,
        pending_user: None,
        pending_chat: None,
    });
    Ok(format!(
        "https://t.me/{}?start={code}",
        loaded.data.username
    ))
}
fn transport_error(error: ImError) -> ApiError {
    ApiError::Unavailable(error.to_string())
}
fn runtime_transport_error(state: &AppState, error: ImError) -> ApiError {
    if let Some(seconds) = error.retry_after_secs() {
        let until = (Utc::now().timestamp().max(0) as u64).saturating_add(seconds);
        state.im_backoff_until.fetch_max(until, Ordering::SeqCst);
    }
    transport_error(error)
}
fn rate_limited(state: &AppState) -> bool {
    state.im_backoff_until.load(Ordering::SeqCst) > Utc::now().timestamp().max(0) as u64
}

impl AppState {
    /// Two independent loops let progress/approvals flow while getUpdates long-polls.
    pub fn start_im_worker(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !matches!(self.interactive_auth, InteractiveAuth::DevelopmentToken(_)) {
            return None;
        }
        let state = self.clone();
        Some(tokio::spawn(async move {
            tokio::join!(poll_loop(&state), delivery_loop(&state));
        }))
    }
}
async fn configured(
    state: &AppState,
) -> Result<Option<(Scope, Loaded, TelegramChannel)>, ApiError> {
    let scope = current_scope(state).await?;
    let loaded = Loaded::load(state, &scope).await?;
    if loaded.data.token.is_empty() {
        return Ok(None);
    }
    let channel = TelegramChannel::new(loaded.data.token.clone()).map_err(transport_error)?;
    Ok(Some((scope, loaded, channel)))
}
async fn poll_loop(state: &AppState) {
    let mut failures = 0u32;
    loop {
        if rate_limited(state) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let result = async {
            let Some((scope, snapshot, channel)) = configured(state).await? else {
                tokio::time::sleep(Duration::from_secs(2)).await;
                return Ok::<_, ApiError>(());
            };
            let poll = channel
                .poll(snapshot.data.offset)
                .await
                .map_err(|e| runtime_transport_error(state, e))?;
            for update in poll.updates {
                let _guard = state.im_lock.lock().await;
                if !same_actor(&scope, &current_scope(state).await?) {
                    break;
                }
                let mut loaded = Loaded::load(state, &scope).await?;
                if loaded.data.generation != snapshot.data.generation
                    || loaded.data.token.is_empty()
                {
                    break;
                }
                if update.update_id < loaded.data.offset {
                    continue;
                }
                process_claimed_update(state, &scope, &mut loaded, &channel, update).await?;
            }
            let _guard = state.im_lock.lock().await;
            if same_actor(&scope, &current_scope(state).await?) {
                let mut loaded = Loaded::load(state, &scope).await?;
                if loaded.data.generation == snapshot.data.generation
                    && poll.next_offset > loaded.data.offset
                {
                    loaded.data.offset = poll.next_offset;
                    loaded.save(state, &scope).await?;
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            failures = (failures + 1).min(5);
            tokio::time::sleep(Duration::from_secs(2u64.pow(failures))).await;
        } else {
            failures = 0;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}
async fn process_claimed_update(
    state: &AppState,
    scope: &Scope,
    loaded: &mut Loaded,
    channel: &dyn ImChannel,
    update: ImUpdate,
) -> Result<(), ApiError> {
    if update.update_id < loaded.data.offset {
        return Ok(());
    }
    // Persist receipt before effects; uncertain dispatch is never auto-replayed.
    loaded.data.offset = update.update_id.saturating_add(1);
    loaded.data.interrupted_update = loaded
        .data
        .binding
        .as_ref()
        .is_some_and(|b| b.user_id == update.user_id && b.chat_id == update.chat_id);
    loaded.save(state, scope).await?;
    if process_update(state, scope, loaded, channel, update)
        .await
        .is_err()
    {
        loaded.queue("This request could not finish. Check the task in S-Code before retrying; it was not automatically repeated.");
    }
    loaded.data.interrupted_update = false;
    loaded.save(state, scope).await
}
async fn process_update(
    state: &AppState,
    scope: &Scope,
    loaded: &mut Loaded,
    channel: &dyn ImChannel,
    update: ImUpdate,
) -> Result<(), ApiError> {
    if loaded.data.binding.is_none() {
        if let ImUpdateKind::Message { text } = &update.kind
            && let Some(code) = text.strip_prefix("/start ")
            && let Some(pair) = &mut loaded.data.pairing
            && pair.expires > Utc::now().timestamp()
            && pair.pending_user.is_none()
            && pair.code_hash == digest(code.trim())
        {
            pair.pending_user = Some(update.user_id);
            pair.pending_chat = Some(update.chat_id);
            loaded.save(state, scope).await?;
            // This single reply is only sent after presenting the local, unguessable link.
            channel.send(update.chat_id,"Pairing requested. On your computer, run s-code im telegram status, verify your user ID, then approve it.",vec![]).await.map_err(|e|runtime_transport_error(state,e))?;
        }
        return Ok(());
    }
    let binding = loaded.data.binding.as_ref().unwrap();
    if binding.user_id != update.user_id || binding.chat_id != update.chat_id {
        return Ok(());
    }
    match update.kind {
        ImUpdateKind::Callback {
            id,
            data,
            message_id,
        } => {
            let result = apply_button(state, scope, loaded, &data, message_id).await;
            let answer = if result.is_ok() {
                "Decision received"
            } else {
                "This button is invalid or expired. Use /status."
            };
            if let Err(error) = channel.answer_callback(&id, answer).await {
                let _ = runtime_transport_error(state, error);
            }
            result?;
        }
        ImUpdateKind::Message { text } => {
            let text = text.trim();
            match text {
                "/start"|"/help" => loaded.queue("S-Code remote tasks\n/sessions — authorized sessions\n/use <session-id> — select a session\n/status — task progress\n/stop — stop the current task\nSend text to start or continue a task. Approve actions only with the task's buttons. Manage access on your computer with s-code im telegram."),
                "/sessions" => {
                    let mut lines=vec!["Authorized sessions:".to_owned()];
                    for id in &loaded.data.sessions {
                        if let Ok(s)=state.store.get_session(id).await && same_actor(scope,&s.scope) && s.status==SessionStatus::Active {
                            lines.push(format!("{} — {}",s.id.0,truncate_text(&s.title,120)));
                        }
                    }
                    if lines.len()==1{lines.push("None yet. On your computer: s-code im telegram allow <session-id>".into());}
                    loaded.queue(lines.join("\n"));
                }
                "/status" => {
                    let binding=loaded.data.binding.as_mut().unwrap();
                    if let Some(active)=&mut binding.active{active.rendered.clear();}
                    else {loaded.queue("No tracked task is running. Select a session with /use, then send a task.");}
                }
                "/stop" => {
                    let active=loaded.data.binding.as_ref().and_then(|b|b.active.as_ref());
                    if let Some(turn)=active.and_then(|a|a.turn_id.clone()){
                        let _ = cancel_turn(State(state.clone()),internal_headers(state)?,Path(turn.0),Json(CancelTurn{scope:scope.clone()})).await?;
                        loaded.queue("Stop requested. Completed changes remain in your workspace.");
                    } else {loaded.queue("No tracked running task to stop. Check S-Code if delivery was interrupted.");}
                }
                _ if text.starts_with("/use ") => {
                    let selected=Id(text[5..].trim().to_owned());
                    if !loaded.data.sessions.contains(&selected){loaded.queue("That session is not authorized. Allow it on your computer first.");return Ok(());}
                    let session=state.store.get_session(&selected).await?;
                    if !same_actor(scope,&session.scope)||session.status!=SessionStatus::Active{return Err(ApiError::Forbidden);}
                    if loaded.data.binding.as_ref().is_some_and(|b|b.active.is_some()){
                        loaded.queue("Wait for the current task or stop it before switching sessions.");return Ok(());
                    }
                    loaded.data.binding.as_mut().unwrap().selected=Some(selected);
                    loaded.queue(format!("Selected: {}",session.title));
                }
                _ if text.starts_with('/') => loaded.queue("Unknown command. Use /help."),
                _ => start_message(state,scope,loaded,text).await?,
            }
        }
    }
    Ok(())
}
async fn start_message(
    state: &AppState,
    scope: &Scope,
    loaded: &mut Loaded,
    text: &str,
) -> Result<(), ApiError> {
    if text.is_empty() || text.len() > 16000 {
        loaded.queue("Send a text task up to 16 KB.");
        return Ok(());
    }
    let binding = loaded.data.binding.as_ref().unwrap();
    if binding.active.is_some() {
        loaded.queue("A task is already active. Use /status or /stop; send your next instruction after it finishes.");
        return Ok(());
    }
    let Some(session_id) = binding
        .selected
        .clone()
        .filter(|id| loaded.data.sessions.contains(id))
    else {
        loaded.queue("Choose an authorized session with /sessions and /use <session-id>.");
        return Ok(());
    };
    let session = state.store.get_session(&session_id).await?;
    if !same_actor(scope, &session.scope) || session.status != SessionStatus::Active {
        return Err(ApiError::Forbidden);
    }
    loaded.data.binding.as_mut().unwrap().active = Some(Delivery {
        session_id: session_id.clone(),
        turn_id: None,
        message_id: None,
        rendered: String::new(),
        approval: None,
    });
    loaded.save(state, scope).await?;
    let (_, Json(turn)) = create_turn(
        State(state.clone()),
        internal_headers(state)?,
        Path(session_id.0),
        Json(CreateTurn {
            scope: scope.clone(),
            content: json!(text),
            attachment_ids: vec![],
            generate_title: false,
        }),
    )
    .await?;
    loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .turn_id = Some(turn.id);
    loaded.save(state, scope).await?;
    Ok(())
}
fn terminal(status: &TurnStatus) -> bool {
    matches!(
        status,
        TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
    )
}
async fn apply_button(
    state: &AppState,
    scope: &Scope,
    loaded: &mut Loaded,
    data: &str,
    message_id: i64,
) -> Result<(), ApiError> {
    let (action, nonce) = data.split_once(':').ok_or(ApiError::Forbidden)?;
    if !matches!(action, "yes" | "no") {
        return Err(ApiError::Forbidden);
    }
    let active = loaded
        .data
        .binding
        .as_ref()
        .and_then(|b| b.active.as_ref())
        .ok_or(ApiError::Forbidden)?;
    let grant = active.approval.as_ref().ok_or(ApiError::Forbidden)?;
    if (action == "yes" && !grant.can_approve)
        || grant.expires <= Utc::now().timestamp()
        || grant.nonce != nonce
        || active.message_id != Some(message_id)
        || !loaded.data.sessions.contains(&active.session_id)
    {
        return Err(ApiError::Forbidden);
    }
    let approval = state.store.get_approval(&grant.approval_id).await?;
    let call = state.store.get_tool_call(&approval.tool_call_id).await?;
    if action == "yes" && call.request.tool != "apply_patch" {
        return Err(ApiError::Forbidden);
    }
    if !same_actor(&approval.scope, scope)
        || approval.status != ApprovalStatus::Pending
        || call.request.session_id != active.session_id
        || Some(&call.request.turn_id) != active.turn_id.as_ref()
    {
        return Err(ApiError::Forbidden);
    }
    let turn = state.store.get_turn(scope, &call.request.turn_id).await?;
    if turn.status != TurnStatus::AwaitingApproval {
        return Err(ApiError::Forbidden);
    }
    if digest(&serde_json::to_string(&call.request).map_err(|_| ApiError::Forbidden)?)
        != grant.digest
    {
        return Err(ApiError::Forbidden);
    }
    let id = grant.approval_id.clone();
    loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap()
        .approval = None;
    loaded.save(state, scope).await?; // Consume before execution, including uncertain failures.
    let _ = resolve_approval(
        State(state.clone()),
        internal_headers(state)?,
        Path(id.0),
        Json(ResolveApproval {
            scope: scope.clone(),
            approved: action == "yes",
            approval_scope: s_code_protocol::ApprovalScope::Once,
        }),
    )
    .await?;
    Ok(())
}
async fn delivery_loop(state: &AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if rate_limited(state) {
            continue;
        }
        let result = async {
            let _guard = state.im_lock.lock().await;
            let Some((scope, mut loaded, channel)) = configured(state).await? else {
                return Ok::<_, ApiError>(());
            };
            if loaded.data.binding.is_none() {
                return Ok(());
            }
            if let Some(text) = loaded.data.outbox.first().cloned() {
                channel
                    .send(loaded.data.binding.as_ref().unwrap().chat_id, &text, vec![])
                    .await
                    .map_err(|e| runtime_transport_error(state, e))?;
                loaded.data.outbox.remove(0);
                loaded.save(state, &scope).await?;
            } else {
                refresh_delivery(state, &scope, &mut loaded, &channel).await?;
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
}
async fn refresh_delivery(
    state: &AppState,
    scope: &Scope,
    loaded: &mut Loaded,
    channel: &dyn ImChannel,
) -> Result<(), ApiError> {
    if loaded.data.interrupted_update {
        loaded.queue("A phone request was interrupted during delivery. Check the selected session before retrying; S-Code will not repeat it automatically.");
        loaded.data.interrupted_update = false;
        loaded.save(state, scope).await?;
    }
    let Some(binding) = loaded.data.binding.as_ref() else {
        return Ok(());
    };
    let chat_id = binding.chat_id;
    let Some(active) = binding.active.as_ref() else {
        return Ok(());
    };
    if !loaded.data.sessions.contains(&active.session_id) {
        return Ok(());
    }
    let Some(turn_id) = active.turn_id.clone() else {
        loaded.queue("Task delivery was interrupted. Check the selected session in S-Code before resending. The task will not be automatically repeated.");
        loaded.data.binding.as_mut().unwrap().active = None;
        loaded.save(state, scope).await?;
        return Ok(());
    };
    let session_id = active.session_id.clone();
    let session = state.store.get_session(&session_id).await?;
    if !same_actor(&session.scope, scope) || session.status != SessionStatus::Active {
        loaded.data.binding.as_mut().unwrap().active = None;
        loaded.save(state, scope).await?;
        return Ok(());
    }
    let turn = state.store.get_turn(scope, &turn_id).await?;
    let mut text = format!(
        "S-Code · {}\n{}",
        truncate_text(&session.title, 120),
        status_label(&turn.status)
    );
    let mut buttons = vec![];
    let active = loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap();
    if turn.status == TurnStatus::AwaitingApproval {
        let pending = state
            .store
            .list_pending_session_approval_calls(scope, &session_id)
            .await?;
        if let Some((approval, call)) = pending
            .into_iter()
            .find(|(_, call)| call.request.turn_id == turn_id)
        {
            let card = transcript_approval_request(&approval, &call);
            let arguments = redact(call.request.arguments.clone());
            let full_arguments = serde_json::to_string_pretty(&arguments)
                .map_err(|_| ApiError::Internal("cannot render operation".into()))?;
            let preview = format!(
                "Tool: {}\n{}\n{}\n\n{}",
                card.tool, card.impact_scope, card.policy_reason, full_arguments
            );
            // Only enable a remote grant when the whole supported operation fits,
            // without concealing values through either truncation or redaction.
            // Command approval executes synchronously in the existing service;
            // keep it local until it has a cancellable background owner.
            let can_approve = card.tool == "apply_patch"
                && arguments == call.request.arguments
                && preview.encode_utf16().count() <= 2400;
            let request_digest = digest(
                &serde_json::to_string(&call.request)
                    .map_err(|_| ApiError::Internal("invalid approval".into()))?,
            );
            if !active.approval.as_ref().is_some_and(|g| {
                g.approval_id == approval.id
                    && g.expires > Utc::now().timestamp()
                    && g.digest == request_digest
            }) {
                active.approval = Some(ButtonGrant {
                    can_approve,
                    nonce: secret()?,
                    approval_id: approval.id,
                    digest: request_digest,
                    expires: Utc::now().timestamp() + 600,
                });
            }
            active.approval.as_mut().unwrap().can_approve = can_approve;
            let grant = active.approval.as_ref().unwrap();
            if can_approve {
                text.push_str(&format!(
                    "\n\n{preview}\n\nApprove once? Buttons expire in 10 minutes."
                ));
                buttons.push(vec![
                    ImButton {
                        text: "Approve once".into(),
                        data: format!("yes:{}", grant.nonce),
                    },
                    ImButton {
                        text: "Reject".into(),
                        data: format!("no:{}", grant.nonce),
                    },
                ]);
            } else {
                let summary = redact(json!({"text":card.summary}));
                text.push_str(&format!(
                    "\n\n{}\n\nSummary only. Review the full operation in S-Code to approve it.",
                    truncate_text(
                        summary["text"]
                            .as_str()
                            .unwrap_or("Operation requires review"),
                        1800
                    )
                ));
                buttons.push(vec![ImButton {
                    text: "Reject".into(),
                    data: format!("no:{}", grant.nonce),
                }]);
            }
        }
    } else {
        active.approval = None;
    }
    if terminal(&turn.status) {
        let messages = state.store.list_messages(scope, &session_id).await?;
        if let Some(message) = messages
            .iter()
            .rev()
            .find(|m| m.turn_id == turn_id && m.role == "assistant")
        {
            let value = redact(message.content.clone());
            let answer = value
                .as_str()
                .or_else(|| value.get("text").and_then(Value::as_str))
                .unwrap_or("");
            text.push_str("\n\n");
            text.push_str(&truncate_text(answer, 3200));
            if answer.encode_utf16().count() > 3200 {
                text.push_str("\n[Full response is available in S-Code.]");
            }
        }
    } else if turn.status == TurnStatus::AwaitingInput {
        text.push_str("\nOpen this session in S-Code to answer the agent's question.");
    } else if turn.status == TurnStatus::RunningTool {
        let calls = state.store.list_tool_calls(scope, &session_id).await?;
        if let Some(call) = calls.iter().rev().find(|c| c.request.turn_id == turn_id) {
            text.push_str(&format!(
                "\nTool: {}",
                truncate_text(&call.request.tool, 160)
            ));
        }
    }
    let rendering=serde_json::to_string(&json!({"text":text,"buttons":buttons.iter().flatten().map(|b|&b.data).collect::<Vec<_>>() })).unwrap();
    if rendering == active.rendered {
        return Ok(());
    }
    let message_id = active.message_id;
    loaded.save(state, scope).await?; // Persist button capabilities before sending them.
    let id = if let Some(id) = message_id {
        match channel.edit(chat_id, id, &text, buttons.clone()).await {
            Ok(()) => id,
            Err(ImError::MessageNotFound) => channel
                .send(chat_id, &text, buttons)
                .await
                .map_err(|e| runtime_transport_error(state, e))?,
            Err(error) => return Err(runtime_transport_error(state, error)),
        }
    } else {
        channel
            .send(chat_id, &text, buttons)
            .await
            .map_err(|e| runtime_transport_error(state, e))?
    };
    let active = loaded
        .data
        .binding
        .as_mut()
        .unwrap()
        .active
        .as_mut()
        .unwrap();
    active.message_id = Some(id);
    active.rendered = rendering;
    if terminal(&turn.status) {
        loaded.data.binding.as_mut().unwrap().active = None;
    }
    loaded.save(state, scope).await?;
    Ok(())
}
fn status_label(status: &TurnStatus) -> &'static str {
    match status {
        TurnStatus::Idle | TurnStatus::PreparingContext => "Preparing your task…",
        TurnStatus::CallingModel => "Thinking…",
        TurnStatus::RunningTool => "Using a tool…",
        TurnStatus::AwaitingApproval => "Your approval is needed",
        TurnStatus::AwaitingInput => "Your answer is needed",
        TurnStatus::Completed => "Task completed",
        TurnStatus::Failed => "Task failed. Check details in S-Code.",
        TurnStatus::Cancelled => "Task stopped",
    }
}
