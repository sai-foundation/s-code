use crate::{input::InputBuffer, tool_display};
use chrono::{DateTime, Utc};
use s_code_protocol::{
    AttachmentMetadata, BackgroundTerminalPreview, BackgroundTerminalSpec, Id, Message,
    PermissionMode, QuestionAnswer, QuestionPrompt, QuestionRequest, QuestionStatus, Session,
    SessionGoal, SessionStatus, SessionUsage, TranscriptPlanStep, TurnInput,
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) mod reducer;

#[derive(Clone, Debug)]
pub(crate) struct LiveEvent {
    pub(crate) id: u64,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) kind: String,
    pub(crate) payload: Value,
    pub(crate) session_id: Option<Id>,
    pub(crate) turn_id: Option<Id>,
    pub(crate) item_id: Option<Id>,
}

pub(crate) struct App {
    pub(crate) sessions: Vec<Session>,
    pub(crate) selected: usize,
    pub(crate) messages: Vec<Message>,
    pub(crate) transcript_item_order: Vec<Id>,
    pub(crate) message_attachments: HashMap<Id, Vec<AttachmentMetadata>>,
    pub(crate) pending_attachments: Vec<AttachmentMetadata>,
    pub(crate) input: InputBuffer,
    pub(crate) activity: VecDeque<String>,
    pub(crate) tool_activity: Vec<ToolActivity>,
    pub(crate) plans: Vec<PlanActivity>,
    pub(crate) notices: Vec<NoticeActivity>,
    pub(crate) usage: SessionUsage,
    pub(crate) usage_turns: HashSet<Id>,
    pub(crate) questions: Vec<QuestionActivity>,
    pub(crate) artifacts: Vec<ArtifactActivity>,
    pub(crate) approvals: VecDeque<ApprovalRequest>,
    pub(crate) approval_selected: usize,
    pub(crate) tool_result: String,
    pub(crate) tool_result_expanded: bool,
    pub(crate) status: String,
    pub(crate) agent_enabled: bool,
    pub(crate) undo_enabled: bool,
    pub(crate) current_turn: Option<Id>,
    pub(crate) turn_running: bool,
    pub(crate) input_mode: InputMode,
    pub(crate) pending_workspace: Option<String>,
    pub(crate) prompt_history: Vec<String>,
    pub(crate) pending_inputs: VecDeque<TurnInput>,
    pub(crate) history_cursor: Option<usize>,
    pub(crate) permission_mode: PermissionMode,
    pub(crate) assistant_alias: String,
    pub(crate) goal: Option<SessionGoal>,
    pub(crate) picker: Option<PickerState>,
    pub(crate) event_cursor: u64,
    pub(crate) pending_terminal: Option<(BackgroundTerminalSpec, BackgroundTerminalPreview)>,
    pub(crate) theme: CliTheme,
    pub(crate) keymap: Keymap,
    pub(crate) vim_mode: VimMode,
    pub(crate) statusline: StatuslineMode,
    pub(crate) slash_command_selected: usize,
    pub(crate) slash_command_dismissed: bool,
    pub(crate) transcript_scroll: usize,
    pub(crate) transcript_next_cursor: Option<String>,
    pub(crate) transcript_loaded_items: u64,
    pub(crate) transcript_item_count: u64,
    pub(crate) editor: Option<String>,
    pub(crate) pending_editor: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApprovalRequest {
    pub(crate) id: String,
    pub(crate) turn_id: Option<Id>,
    pub(crate) tool: String,
    pub(crate) display: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanActivity {
    pub(crate) id: Id,
    pub(crate) turn_id: Id,
    pub(crate) title: Option<String>,
    pub(crate) steps: Vec<TranscriptPlanStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NoticeActivity {
    pub(crate) item_id: Id,
    pub(crate) turn_id: Id,
    pub(crate) label: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuestionActivity {
    pub(crate) id: Id,
    pub(crate) item_id: Id,
    pub(crate) turn_id: Id,
    pub(crate) questions: Vec<QuestionPrompt>,
    pub(crate) allow_other: bool,
    pub(crate) status: QuestionStatus,
    pub(crate) answers: Vec<QuestionAnswer>,
    pub(crate) expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactActivity {
    pub(crate) item_id: Id,
    pub(crate) artifact_id: Id,
    pub(crate) turn_id: Id,
    pub(crate) title: String,
    pub(crate) media_type: String,
}

impl From<QuestionRequest> for QuestionActivity {
    fn from(request: QuestionRequest) -> Self {
        Self {
            id: request.id,
            item_id: request.item_id,
            turn_id: request.turn_id,
            questions: request.questions,
            allow_other: request.allow_other,
            status: request.status,
            answers: request.answers,
            expires_at: request.expires_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolActivity {
    pub(crate) item_id: Id,
    pub(crate) turn_id: Option<Id>,
    pub(crate) call_id: Option<String>,
    pub(crate) parent_tool_call_id: Option<String>,
    pub(crate) tool: String,
    pub(crate) display: String,
    pub(crate) state: ToolActivityState,
    pub(crate) progress: Option<ToolProgress>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolProgress {
    pub(crate) progress: f64,
    pub(crate) total: Option<f64>,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolActivityState {
    Preparing,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Denied,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputMode {
    Prompt,
    NewWorkspace,
    NewModel,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CliTheme {
    #[default]
    Dark,
    Light,
    NoColor,
}

impl CliTheme {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            "no_color" => Some(Self::NoColor),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::NoColor => "no_color",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Keymap {
    #[default]
    Emacs,
    Vim,
}

impl Keymap {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "emacs" => Some(Self::Emacs),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Emacs => "emacs",
            Self::Vim => "vim",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum VimMode {
    Normal,
    #[default]
    Insert,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StatuslineMode {
    #[default]
    Full,
    Compact,
    Off,
}

impl StatuslineMode {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "compact" => Some(Self::Compact),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerKind {
    History,
    Link,
    Model,
    Permission,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerOption {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) disabled_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerState {
    pub(crate) kind: PickerKind,
    pub(crate) title: String,
    pub(crate) options: Vec<PickerOption>,
    pub(crate) query: String,
    pub(crate) selected: usize,
}

impl PickerState {
    pub(crate) fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter_map(|(index, option)| {
                (query.is_empty()
                    || format!("{} {} {}", option.id, option.label, option.detail)
                        .to_lowercase()
                        .contains(&query))
                .then_some(index)
            })
            .collect()
    }

    pub(crate) fn move_selection(&mut self, direction: isize) {
        let count = self.visible_indices().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + direction).rem_euclid(count as isize) as usize;
    }

    pub(crate) fn selected_option(&self) -> Option<&PickerOption> {
        let index = *self.visible_indices().get(self.selected)?;
        self.options.get(index)
    }
}

impl App {
    pub(crate) fn new(sessions: Vec<Session>, agent_enabled: bool, undo_enabled: bool) -> Self {
        Self {
            sessions,
            selected: 0,
            messages: vec![],
            transcript_item_order: vec![],
            message_attachments: HashMap::new(),
            pending_attachments: Vec::new(),
            input: InputBuffer::default(),
            activity: VecDeque::new(),
            tool_activity: Vec::new(),
            plans: Vec::new(),
            notices: Vec::new(),
            usage: SessionUsage::default(),
            usage_turns: HashSet::new(),
            questions: Vec::new(),
            artifacts: Vec::new(),
            approvals: VecDeque::new(),
            approval_selected: 1,
            tool_result: "Press d to load the current Git diff.".into(),
            tool_result_expanded: false,
            status: if agent_enabled {
                "idle"
            } else {
                "agent unavailable"
            }
            .into(),
            agent_enabled,
            undo_enabled,
            current_turn: None,
            turn_running: false,
            input_mode: InputMode::Prompt,
            pending_workspace: None,
            prompt_history: Vec::new(),
            pending_inputs: VecDeque::new(),
            history_cursor: None,
            permission_mode: PermissionMode::Manual,
            assistant_alias: "S-Code".into(),
            goal: None,
            picker: None,
            event_cursor: 0,
            pending_terminal: None,
            theme: CliTheme::Dark,
            keymap: Keymap::Emacs,
            vim_mode: VimMode::Insert,
            statusline: StatuslineMode::Full,
            slash_command_selected: 0,
            slash_command_dismissed: false,
            transcript_scroll: 0,
            transcript_next_cursor: None,
            transcript_loaded_items: 0,
            transcript_item_count: 0,
            editor: None,
            pending_editor: None,
        }
    }

    pub(crate) fn configure_interface(
        &mut self,
        theme: CliTheme,
        keymap: Keymap,
        statusline: StatuslineMode,
        editor: Option<String>,
    ) {
        self.theme = theme;
        self.keymap = keymap;
        self.vim_mode = VimMode::Insert;
        self.statusline = statusline;
        self.editor = editor;
    }

    pub(crate) fn scroll_transcript_up(&mut self, rows: usize, max_scroll: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(rows).min(max_scroll);
    }

    pub(crate) fn scroll_transcript_down(&mut self, rows: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(rows);
    }

    pub(crate) fn composer_input_changed(&mut self) {
        self.slash_command_selected = 0;
        self.slash_command_dismissed = false;
    }

    pub(crate) fn track_transcript_item(&mut self, item_id: Id) {
        if !self
            .transcript_item_order
            .iter()
            .any(|existing| existing == &item_id)
        {
            self.transcript_item_order.push(item_id);
        }
    }

    fn track_turn_activity_item(&mut self, item_id: Id, turn_id: Option<&Id>) {
        if self
            .transcript_item_order
            .iter()
            .any(|existing| existing == &item_id)
        {
            return;
        }
        let before_streaming_answer = turn_id.and_then(|turn_id| {
            self.transcript_item_order.iter().rposition(|ordered_id| {
                self.messages.iter().any(|message| {
                    &message.id == ordered_id
                        && &message.turn_id == turn_id
                        && message.role == "assistant"
                })
            })
        });
        if let Some(index) = before_streaming_answer {
            self.transcript_item_order.insert(index, item_id);
        } else {
            self.transcript_item_order.push(item_id);
        }
    }

    pub(crate) fn current(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    /// Apply one server event to client state. The reduction is idempotent by
    /// event cursor and all streamed text is stored in the same Message list
    /// used for snapshot history.
    pub(crate) fn apply_event(&mut self, event: LiveEvent) -> bool {
        if event.id <= self.event_cursor {
            return false;
        }
        // Event IDs are database-global while this stream is Team-filtered.
        // Other Teams legitimately create forward jumps. A lagged server-side
        // subscription terminates the stream so the client can replay from
        // this global cursor instead of trying to infer loss from numbering.
        let is_visible_session = event.session_id.is_none()
            || event.session_id.as_ref() == self.current().map(|session| &session.id);
        if is_visible_session
            && event.kind == "model.delta"
            && let Some(received) = event.payload["byte_offset"].as_u64()
        {
            let expected = event
                .item_id
                .as_ref()
                .and_then(|item_id| self.messages.iter().find(|message| &message.id == item_id))
                .and_then(|message| message.content.as_str())
                .map_or(0, |content| {
                    u64::try_from(content.len()).unwrap_or(u64::MAX)
                });
            if received != expected {
                self.status = format!(
                    "transcript append gap · expected byte offset {expected} · received {received} · restoring snapshot"
                );
                return false;
            }
        }
        self.event_cursor = event.id;
        if event.kind == "session.updated"
            && let Some(session_id) = event.session_id.as_ref()
            && let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| &session.id == session_id)
        {
            if let Some(title) = event.payload["title"].as_str() {
                session.title = title.into();
            }
            if let Some(model) = event.payload["model"].as_str() {
                session.model = model.into();
            }
        }
        if event.kind == "session.preferences.updated"
            && event.session_id.as_ref() == self.current().map(|session| &session.id)
            && let Some(mode) = event.payload["permission_mode"].as_str()
        {
            self.permission_mode = match mode {
                "accept_edits" => PermissionMode::AcceptEdits,
                "workspace" => PermissionMode::Workspace,
                "plan" => PermissionMode::Plan,
                _ => PermissionMode::Manual,
            };
        }
        if event.kind == "session.updated"
            && let (Some(session_id), Some(status)) =
                (event.session_id.as_ref(), event.payload["status"].as_str())
            && let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| &session.id == session_id)
        {
            session.status = match status {
                "active" => SessionStatus::Active,
                "archived" => SessionStatus::Archived,
                "deleted" => SessionStatus::Deleted,
                _ => session.status.clone(),
            };
        }
        if event.session_id.is_some()
            && event.session_id.as_ref() != self.current().map(|session| &session.id)
        {
            return true;
        }
        let is_current_turn =
            self.current_turn.is_some() && self.current_turn.as_ref() == event.turn_id.as_ref();
        match event.kind.as_str() {
            "turn.created" => {
                self.current_turn = event.turn_id.clone();
                self.turn_running = true;
                self.status = "starting".into();
            }
            "model.delta" if is_current_turn && self.turn_running => {
                self.apply_message_delta(&event);
            }
            "turn.status" if is_current_turn => {
                self.status = event.payload["status"].as_str().unwrap_or("working").into()
            }
            "turn.completed" | "turn.failed" | "turn.cancelled" if is_current_turn => {
                self.status = event.kind.trim_start_matches("turn.").into();
                self.turn_running = false;
            }
            "approval.required" => {
                if let Some(id) = event.payload["approval_id"].as_str() {
                    let tool = event.payload["tool"].as_str().unwrap_or("tool");
                    if !self.approvals.iter().any(|approval| approval.id == id) {
                        if self.approvals.is_empty() {
                            self.approval_selected = 1;
                        }
                        self.approvals.push_back(ApprovalRequest {
                            id: id.into(),
                            turn_id: event.turn_id.clone(),
                            tool: tool.into(),
                            display: tool_display(&event.payload, tool),
                        });
                    }
                }
                if self.current_turn.is_none() {
                    self.current_turn = event.turn_id.clone();
                    self.turn_running = true;
                }
            }
            "tool.completed" | "tool.failed" | "tool.denied" | "tool.cancelled" => {
                let tool = event.payload["tool"].as_str().unwrap_or("tool");
                let previous_front = self.approvals.front().map(|approval| approval.id.clone());
                self.approvals
                    .retain(|approval| approval.turn_id != event.turn_id || approval.tool != tool);
                if self.approvals.front().map(|approval| &approval.id) != previous_front.as_ref() {
                    self.approval_selected = 1;
                }
            }
            "plan.updated" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                    && let Ok(steps) = serde_json::from_value::<Vec<TranscriptPlanStep>>(
                        event
                            .payload
                            .get("steps")
                            .cloned()
                            .unwrap_or_else(|| json!([])),
                    )
                {
                    let title = event
                        .payload
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.track_transcript_item(item_id.clone());
                    if let Some(plan) = self.plans.iter_mut().find(|plan| plan.id == item_id) {
                        plan.title = title;
                        plan.steps = steps;
                    } else {
                        self.plans.push(PlanActivity {
                            id: item_id,
                            turn_id,
                            title,
                            steps,
                        });
                    }
                }
            }
            "context.compacted" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                {
                    let omitted = event.payload["omitted_messages"]
                        .as_u64()
                        .unwrap_or_default();
                    let truncated = event.payload["truncated_messages"]
                        .as_u64()
                        .unwrap_or_default();
                    let tokens = event.payload["estimated_tokens"]
                        .as_u64()
                        .unwrap_or_default();
                    let mut parts = vec![format!(
                        "{omitted} earlier message{} summarized",
                        if omitted == 1 { "" } else { "s" }
                    )];
                    if truncated > 0 {
                        parts.push(format!(
                            "{truncated} long message{} shortened",
                            if truncated == 1 { "" } else { "s" }
                        ));
                    }
                    if tokens > 0 {
                        parts.push(format!("about {tokens} summary tokens"));
                    }
                    self.upsert_notice(NoticeActivity {
                        item_id,
                        turn_id,
                        label: "Context optimized".into(),
                        detail: parts.join(" · "),
                    });
                }
            }
            "model.route.fallback" => {
                if let Some(turn_id) = event.turn_id.clone() {
                    let item_id = event
                        .item_id
                        .clone()
                        .unwrap_or_else(|| Id(event.id.to_string()));
                    let from = event.payload["from_model_id"].as_str();
                    let to = event.payload["to_model_id"]
                        .as_str()
                        .unwrap_or("fallback model");
                    let route = from.map_or_else(|| to.to_owned(), |from| format!("{from} → {to}"));
                    let reason = event.payload["fallback_reason"]
                        .as_str()
                        .unwrap_or("routing policy");
                    self.upsert_notice(NoticeActivity {
                        item_id,
                        turn_id,
                        label: "Model switched".into(),
                        detail: format!("{route} · {reason}"),
                    });
                }
            }
            "reasoning.summary.delta" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                {
                    self.track_transcript_item(item_id.clone());
                    let delta = event.payload["text"].as_str().unwrap_or_default();
                    if let Some(existing) = self
                        .notices
                        .iter_mut()
                        .find(|notice| notice.item_id == item_id)
                    {
                        existing.detail.push_str(delta);
                    } else {
                        self.notices.push(NoticeActivity {
                            item_id,
                            turn_id,
                            label: "Reasoning summary".into(),
                            detail: delta.into(),
                        });
                    }
                }
            }
            "turn.usage" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                {
                    let input_tokens = event.payload["input_units"].as_u64().unwrap_or_default();
                    let output_tokens = event.payload["output_units"].as_u64().unwrap_or_default();
                    let model_calls = event.payload["model_calls"].as_u64().unwrap_or_default();
                    let tool_calls = event.payload["tool_calls"].as_u64().unwrap_or_default();
                    self.usage.input_tokens = self.usage.input_tokens.saturating_add(input_tokens);
                    self.usage.output_tokens =
                        self.usage.output_tokens.saturating_add(output_tokens);
                    self.usage.total_tokens = self
                        .usage
                        .input_tokens
                        .saturating_add(self.usage.output_tokens);
                    self.usage.model_calls = self.usage.model_calls.saturating_add(model_calls);
                    self.usage.tool_calls = self.usage.tool_calls.saturating_add(tool_calls);
                    if self.usage_turns.insert(turn_id.clone()) {
                        self.usage.turns = self.usage.turns.saturating_add(1);
                    }
                    let model = event.payload["model"].as_str().unwrap_or("model");
                    self.upsert_notice(NoticeActivity {
                        item_id,
                        turn_id,
                        label: "Usage".into(),
                        detail: format!(
                            "{} tokens · {} input + {} output · {} model / {} tool calls · {}",
                            input_tokens.saturating_add(output_tokens),
                            input_tokens,
                            output_tokens,
                            model_calls,
                            tool_calls,
                            model,
                        ),
                    });
                }
            }
            "mcp.progress" => self.apply_mcp_progress_event(&event),
            "agent.status" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                {
                    let label = event
                        .payload
                        .get("label")
                        .and_then(Value::as_str)
                        .or_else(|| event.payload.get("status").and_then(Value::as_str))
                        .unwrap_or("updated")
                        .to_owned();
                    self.upsert_notice(NoticeActivity {
                        item_id,
                        turn_id,
                        label: "Agent".into(),
                        detail: label,
                    });
                }
            }
            "hook.started" | "hook.completed" | "hook.failed" => {
                if let (Some(turn_id), Some(item_id)) =
                    (event.turn_id.clone(), event.item_id.clone())
                {
                    let phase = event
                        .payload
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or("hook");
                    let handler = event
                        .payload
                        .get("handler")
                        .and_then(Value::as_str)
                        .unwrap_or("handler");
                    let mut detail = format!("{phase} · {handler}");
                    if event
                        .payload
                        .get("input_modified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        detail.push_str(" · input modified");
                    }
                    if let Some(summary) =
                        event.payload.get("result_summary").and_then(Value::as_str)
                    {
                        detail.push_str(&format!(" · {summary}"));
                    } else if let Some(code) =
                        event.payload.get("error_code").and_then(Value::as_str)
                    {
                        detail.push_str(&format!(" · failed: {code}"));
                    }
                    self.upsert_notice(NoticeActivity {
                        item_id,
                        turn_id,
                        label: "Hook".into(),
                        detail,
                    });
                }
            }
            "question.required" => {
                if let (Some(turn_id), Some(item_id), Some(request_id)) = (
                    event.turn_id.clone(),
                    event.item_id.clone(),
                    event
                        .payload
                        .get("request_id")
                        .and_then(Value::as_str)
                        .map(|value| Id(value.into())),
                ) && let Ok(questions) = serde_json::from_value::<Vec<QuestionPrompt>>(
                    event
                        .payload
                        .get("questions")
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                ) && !self
                    .questions
                    .iter()
                    .any(|question| question.id == request_id)
                {
                    self.track_transcript_item(item_id.clone());
                    self.questions.push(QuestionActivity {
                        id: request_id,
                        item_id,
                        turn_id,
                        questions,
                        allow_other: event
                            .payload
                            .get("allow_other")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        status: QuestionStatus::Pending,
                        answers: Vec::new(),
                        expires_at: event
                            .payload
                            .get("expires_at")
                            .and_then(Value::as_str)
                            .and_then(|value| value.parse().ok()),
                    });
                }
                self.status = "waiting for your answer".into();
                self.turn_running = true;
            }
            "question.answered" => {
                if let Some(request_id) = event.payload.get("request_id").and_then(Value::as_str)
                    && let Some(question) = self
                        .questions
                        .iter_mut()
                        .find(|question| question.id.0 == request_id)
                {
                    question.status = QuestionStatus::Answered;
                }
            }
            "artifact.created" => {
                if let (Some(turn_id), Some(item_id), Some(artifact_id)) = (
                    event.turn_id.clone(),
                    event.item_id.clone(),
                    event
                        .payload
                        .get("artifact_id")
                        .and_then(Value::as_str)
                        .map(|value| Id(value.into())),
                ) {
                    let title = event
                        .payload
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Artifact")
                        .to_owned();
                    let media_type = event
                        .payload
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream")
                        .to_owned();
                    if !self
                        .artifacts
                        .iter()
                        .any(|artifact| artifact.artifact_id == artifact_id)
                    {
                        self.track_transcript_item(item_id.clone());
                        self.artifacts.push(ArtifactActivity {
                            item_id,
                            artifact_id,
                            turn_id,
                            title,
                            media_type,
                        });
                    }
                }
            }
            _ => {}
        }
        if !matches!(
            event.payload["tool"].as_str(),
            Some("update_plan" | "request_user_input" | "publish_artifact")
        ) {
            self.apply_tool_event(&event);
        }
        true
    }

    fn apply_message_delta(&mut self, event: &LiveEvent) {
        let Some(turn_id) = event.turn_id.clone() else {
            return;
        };
        let Some(session_id) = event
            .session_id
            .clone()
            .or_else(|| self.current().map(|session| session.id.clone()))
        else {
            return;
        };
        let item_id = event
            .item_id
            .clone()
            .unwrap_or_else(|| Id(format!("streaming-{}", turn_id.0)));
        self.track_transcript_item(item_id.clone());
        let delta = event.payload["text"].as_str().unwrap_or("");
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == item_id)
        {
            let content = message.content.as_str().unwrap_or("").to_owned() + delta;
            message.content = Value::String(content);
        } else {
            self.messages.push(Message {
                id: item_id,
                session_id,
                turn_id,
                role: "assistant".into(),
                content: json!(delta),
                created_at: event.timestamp,
            });
        }
    }

    fn apply_tool_event(&mut self, event: &LiveEvent) {
        let state = match event.kind.as_str() {
            "tool.proposed" => ToolActivityState::Preparing,
            "tool.running" => ToolActivityState::Running,
            "approval.required" => ToolActivityState::AwaitingApproval,
            "tool.completed" => ToolActivityState::Completed,
            "tool.failed" => ToolActivityState::Failed,
            "tool.denied" => ToolActivityState::Denied,
            "tool.cancelled" => ToolActivityState::Cancelled,
            _ => return,
        };
        let tool = event.payload["tool"].as_str().unwrap_or("tool");
        let call_id = event.payload["tool_call_id"]
            .as_str()
            .or_else(|| event.payload["model_call_id"].as_str())
            .map(str::to_owned)
            .or_else(|| event.item_id.as_ref().map(|id| id.0.clone()));
        let exact = self.tool_activity.iter().rposition(|item| {
            item.turn_id == event.turn_id
                && item.call_id.is_some()
                && item.call_id.as_ref() == call_id.as_ref()
        });
        let parent_tool_call_id = event.payload["parent_tool_call_id"]
            .as_str()
            .map(str::to_owned);
        let pending = self.tool_activity.iter().rposition(|item| {
            event.kind != "tool.proposed"
                && parent_tool_call_id.is_none()
                && item.parent_tool_call_id.is_none()
                && item.turn_id == event.turn_id
                && item.tool == tool
                && item.state == ToolActivityState::Preparing
        });
        let item_id = if let Some(index) = exact.or(pending) {
            let item = &mut self.tool_activity[index];
            if item.state == ToolActivityState::Cancelled {
                return;
            }
            item.call_id = call_id.or_else(|| item.call_id.clone());
            if event.payload["display"].as_str().is_some() {
                item.display = tool_display(&event.payload, tool);
            }
            item.state = state;
            item.item_id.clone()
        } else {
            let item_id = event
                .item_id
                .clone()
                .or_else(|| call_id.as_ref().map(|value| Id(value.clone())))
                .unwrap_or_else(|| Id(format!("event-{}", event.id)));
            self.tool_activity.push(ToolActivity {
                item_id: item_id.clone(),
                turn_id: event.turn_id.clone(),
                call_id,
                parent_tool_call_id,
                tool: tool.into(),
                display: tool_display(&event.payload, tool),
                state,
                progress: None,
            });
            item_id
        };
        self.track_turn_activity_item(item_id, event.turn_id.as_ref());
        if self.tool_activity.len() > 200 {
            self.tool_activity.drain(..self.tool_activity.len() - 200);
        }
    }

    fn apply_mcp_progress_event(&mut self, event: &LiveEvent) {
        let call_id = event.payload["tool_call_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| event.item_id.as_ref().map(|id| id.0.clone()));
        let progress = ToolProgress {
            progress: event.payload["progress"].as_f64().unwrap_or_default(),
            total: event.payload["total"].as_f64(),
            message: event.payload["message"].as_str().map(str::to_owned),
        };
        if let Some(index) = self.tool_activity.iter().rposition(|item| {
            item.turn_id == event.turn_id
                && item.call_id.is_some()
                && item.call_id.as_ref() == call_id.as_ref()
        }) {
            let item_id = {
                let item = &mut self.tool_activity[index];
                item.state = ToolActivityState::Running;
                item.progress = Some(progress);
                item.item_id.clone()
            };
            self.track_turn_activity_item(item_id, event.turn_id.as_ref());
            return;
        }
        let server = event.payload["server"].as_str().unwrap_or("server");
        let tool = event.payload["tool"].as_str().unwrap_or("tool");
        let item_id = event
            .item_id
            .clone()
            .or_else(|| call_id.as_ref().map(|value| Id(value.clone())))
            .unwrap_or_else(|| Id(format!("event-{}", event.id)));
        self.tool_activity.push(ToolActivity {
            item_id: item_id.clone(),
            turn_id: event.turn_id.clone(),
            call_id,
            parent_tool_call_id: None,
            tool: format!("mcp.{server}.{tool}"),
            display: format!("MCP {server} · {tool}"),
            state: ToolActivityState::Running,
            progress: Some(progress),
        });
        self.track_turn_activity_item(item_id, event.turn_id.as_ref());
    }

    fn upsert_notice(&mut self, notice: NoticeActivity) {
        self.track_transcript_item(notice.item_id.clone());
        if let Some(existing) = self
            .notices
            .iter_mut()
            .find(|existing| existing.item_id == notice.item_id)
        {
            *existing = notice;
        } else {
            self.notices.push(notice);
        }
    }

    pub(crate) fn resolve_tool_approval(&mut self, approval: &ApprovalRequest, approved: bool) {
        if let Some(item) = self.tool_activity.iter_mut().rfind(|item| {
            item.turn_id == approval.turn_id
                && item.tool == approval.tool
                && item.state == ToolActivityState::AwaitingApproval
        }) {
            item.state = if approved {
                ToolActivityState::Running
            } else {
                ToolActivityState::Denied
            };
        }
    }

    pub(crate) fn clear_transcript(&mut self) {
        self.messages.clear();
        self.transcript_item_order.clear();
        self.message_attachments.clear();
        self.tool_activity.clear();
        self.plans.clear();
        self.notices.clear();
        self.usage = SessionUsage::default();
        self.usage_turns.clear();
        self.questions.clear();
        self.artifacts.clear();
        self.approvals.clear();
        self.approval_selected = 1;
        self.current_turn = None;
        self.turn_running = false;
        self.transcript_scroll = 0;
        self.transcript_next_cursor = None;
        self.transcript_loaded_items = 0;
        self.transcript_item_count = 0;
    }
}
