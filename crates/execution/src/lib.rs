mod editing;
pub use editing::editing_paths;

use s_code_git_automation::{CommitRequest, GitService};
use s_code_platform_runtime::PlatformRuntime;
use s_code_policy::{PolicyBundle, applies_to_rollout};
use s_code_protocol::{
    ApprovalStatus, Id, PolicyDecision, ResolveApproval, SubmitToolCall, ToolCall, ToolCallOutcome,
    ToolCallStatus, ToolRequest,
};
use s_code_storage::{StorageError, Store, ToolPolicyMetadata};
use s_code_tool_runtime::{CommandCompatibility, FileReplacement, ToolError, ToolRuntime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

const MAX_TOOL_OUTPUT_BYTES: usize = 1024 * 1024;

fn bounded_tool_output_bytes(requested: Option<usize>, default: usize) -> usize {
    requested.unwrap_or(default).clamp(1, MAX_TOOL_OUTPUT_BYTES)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyPatchArgs {
    path: String,
    expected_sha256: Option<String>,
    expected_revision: Option<String>,
    content: Option<String>,
    #[serde(default)]
    edits: Vec<FileEdit>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEdit {
    old_text: Option<String>,
    start_line: Option<usize>,
    end_line: Option<usize>,
    new_text: String,
    #[serde(skip)]
    patch_hunk: bool,
}

impl ApplyPatchArgs {
    fn validate_shape(&self) -> Result<(), ExecutionError> {
        if self.edits.len() > 100 {
            return Err(ExecutionError::Arguments(
                "at most 100 edits per file".into(),
            ));
        }
        match (self.content.is_some(), self.edits.is_empty()) {
            (true, true) | (false, false) => Ok(()),
            _ => Err(ExecutionError::Arguments(
                "apply_patch requires exactly one of content or non-empty edits".into(),
            )),
        }
    }

    fn into_replacement(self, snapshot: &[u8]) -> Result<FileReplacement, ExecutionError> {
        self.validate_shape()?;
        let content = if let Some(content) = self.content {
            content
        } else {
            let content = String::from_utf8(snapshot.to_vec()).map_err(|_| {
                ExecutionError::Arguments("text edits require an existing UTF-8 text file".into())
            })?;
            apply_file_edits(content, self.edits)?
        };
        Ok(FileReplacement {
            path: self.path,
            expected_sha256: self
                .expected_sha256
                .filter(|value| !value.eq_ignore_ascii_case("null")),
            content,
        })
    }
}

fn apply_file_edits(
    mut content: String,
    mut edits: Vec<FileEdit>,
) -> Result<String, ExecutionError> {
    let line_mode = edits.iter().all(|edit| {
        edit.old_text.is_none() && edit.start_line.is_some() && edit.end_line.is_some()
    });
    let exact_mode = edits.iter().all(|edit| {
        edit.old_text.is_some() && edit.start_line.is_none() && edit.end_line.is_none()
    });
    if !line_mode && !exact_mode {
        return Err(ExecutionError::Arguments(
            "edits must use one mode consistently: old_text, or start_line/end_line".into(),
        ));
    }
    if exact_mode {
        for (index, edit) in edits.into_iter().enumerate() {
            let old_text = edit.old_text.expect("exact edit mode checked");
            if old_text.is_empty() {
                return Err(ExecutionError::Arguments(format!(
                    "edit {} has empty old_text",
                    index + 1
                )));
            }
            // Advance by one UTF-8 character after each match, rather than by
            // the whole needle, so overlapping occurrences remain visible.
            let mut positions = Vec::with_capacity(2);
            let mut search_start = 0;
            while let Some(relative) = content[search_start..].find(&old_text) {
                let offset = search_start + relative;
                if !edit.patch_hunk || offset == 0 || content.as_bytes()[offset - 1] == b'\n' {
                    positions.push(offset);
                    if positions.len() == 2 {
                        break;
                    }
                }
                search_start = offset
                    + content[offset..]
                        .chars()
                        .next()
                        .expect("nonempty match")
                        .len_utf8();
            }
            let matches = positions.len();
            if matches != 1 {
                let recovery = (matches == 0)
                    .then(|| closest_edit_excerpt(&content, &old_text))
                    .flatten()
                    .map(|excerpt| {
                        format!(
                            ". Do not retry the same old_text. Copy an exact block from this closest current excerpt or send a deliberate full content replacement:\n{excerpt}"
                        )
                    })
                    .unwrap_or_default();
                return Err(ExecutionError::Arguments(format!(
                    "edit {} old_text must match exactly once, but matched {matches} times{recovery}",
                    index + 1,
                )));
            }
            content.replace_range(positions[0]..positions[0] + old_text.len(), &edit.new_text);
        }
        return Ok(content);
    }

    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start_line.unwrap_or_default()));
    let mut previous_start = usize::MAX;
    for edit in edits {
        let start_line = edit.start_line.expect("line edit mode checked");
        let end_line = edit.end_line.expect("line edit mode checked");
        let mut starts = vec![0];
        starts.extend(content.match_indices('\n').map(|(index, _)| index + 1));
        let line_count = if content.ends_with('\n') {
            starts.len().saturating_sub(1)
        } else {
            starts.len()
        };
        if start_line == 0 || end_line < start_line || end_line > line_count {
            return Err(ExecutionError::Arguments(format!(
                "line edit range {start_line}-{end_line} is outside the file's 1-{line_count} lines"
            )));
        }
        if end_line >= previous_start {
            return Err(ExecutionError::Arguments(
                "line edit ranges must not overlap".into(),
            ));
        }
        previous_start = start_line;
        let start = starts[start_line - 1];
        let end = starts.get(end_line).copied().unwrap_or(content.len());
        let replaced_had_newline = content[start..end].ends_with('\n');
        let mut replacement = edit.new_text;
        if replaced_had_newline && !replacement.is_empty() && !replacement.ends_with('\n') {
            replacement.push('\n');
        }
        content.replace_range(start..end, &replacement);
    }
    Ok(content)
}

fn closest_edit_excerpt(content: &str, old_text: &str) -> Option<String> {
    let sought = old_text
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|token| token.len() >= 2)
        .collect::<HashSet<_>>();
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return None;
    }
    let mut start = 0;
    let mut best_score = 0;
    for candidate in 0..lines.len() {
        let end = (candidate + 7).min(lines.len());
        let score = sought
            .iter()
            .filter(|token| {
                lines[candidate..end]
                    .iter()
                    .any(|line| line.contains(**token))
            })
            .count();
        if score > best_score {
            start = candidate;
            best_score = score;
        }
    }
    let end = (start + 7).min(lines.len());
    let excerpt = lines[start..end].join("\n");
    Some(excerpt.chars().take(1200).collect())
}

fn expected_revision_matches(current_sha256: &str, expected: &str) -> bool {
    current_sha256 == expected
        || (expected.len() == 16
            && expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            && current_sha256.starts_with(expected))
}

fn number_file_content(mut value: Value, start_line: usize) -> Value {
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let Some(content) = object
        .remove("content")
        .and_then(|value| value.as_str().map(str::to_owned))
    else {
        return value;
    };
    let numbered = content
        .lines()
        .enumerate()
        .map(|(index, line)| format!("{}: {line}", start_line + index))
        .collect::<Vec<_>>()
        .join("\n");
    object.insert("numbered_content".into(), Value::String(numbered));
    object.insert(
        "content_format".into(),
        Value::String(
            "absolute line number, colon, space, then file text; prefixes are metadata".into(),
        ),
    );
    value
}

#[async_trait::async_trait]
pub trait ExternalToolExecutor: Send + Sync {
    async fn execute(&self, call: &ToolCall) -> Result<Option<Value>, String>;
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("invalid tool arguments: {0}")]
    Arguments(String),
    #[error("invalid approval state: {0}")]
    Approval(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct UndoTurnResult {
    pub turn_id: Id,
    pub restored_paths: Vec<String>,
    pub conversation_omitted_messages: u32,
    pub plan_items_rewound: u32,
    pub cancelled_input_ids: Vec<Id>,
    pub session_goal_rewound: bool,
}

#[derive(Clone, Debug)]
pub struct PreparedToolCall {
    request: ToolRequest,
    policy: s_code_protocol::PolicyResult,
    metadata: ToolPolicyMetadata,
}

impl PreparedToolCall {
    pub fn decision(&self) -> &PolicyDecision {
        &self.policy.decision
    }
}

#[derive(Clone)]
pub struct ExecutionService {
    store: Store,
    policy: PolicyBundle,
    platform: Arc<dyn PlatformRuntime>,
    external: Option<Arc<dyn ExternalToolExecutor>>,
    device_id: Id,
}

impl ExecutionService {
    pub fn new(store: Store, policy: PolicyBundle, platform: Arc<dyn PlatformRuntime>) -> Self {
        Self {
            store,
            policy,
            platform,
            external: None,
            device_id: Id("device-local".into()),
        }
    }

    pub fn with_external(mut self, external: Arc<dyn ExternalToolExecutor>) -> Self {
        self.external = Some(external);
        self
    }

    pub fn with_device_id(mut self, device_id: Id) -> Self {
        self.device_id = device_id;
        self
    }

    pub async fn submit(
        &self,
        session_id: &Id,
        input: SubmitToolCall,
    ) -> Result<ToolCallOutcome, ExecutionError> {
        let turn = self.store.create_turn(&input.scope, session_id).await?;
        self.store
            .update_turn(
                &input.scope,
                &turn.id,
                s_code_protocol::TurnStatus::PreparingContext,
                None,
                None,
            )
            .await?;
        self.store
            .update_turn(
                &input.scope,
                &turn.id,
                s_code_protocol::TurnStatus::CallingModel,
                None,
                None,
            )
            .await?;
        let marker = serde_json::json!({"execution_mode":"manual_tool"});
        self.store
            .update_turn(
                &input.scope,
                &turn.id,
                s_code_protocol::TurnStatus::RunningTool,
                Some(&marker),
                None,
            )
            .await?;
        let scope = input.scope.clone();
        let outcome = self.submit_for_turn(session_id, &turn.id, input).await?;
        self.finish_manual_turn(&scope, &turn.id, &outcome).await?;
        Ok(outcome)
    }

    pub async fn submit_for_turn(
        &self,
        session_id: &Id,
        turn_id: &Id,
        input: SubmitToolCall,
    ) -> Result<ToolCallOutcome, ExecutionError> {
        let prepared = self.preflight_for_turn(session_id, turn_id, input).await?;
        self.submit_prepared(prepared).await
    }

    /// Validates arguments and resolves all immediate policy decisions without
    /// creating an approval or executing a tool. Callers may safely run cold
    /// interaction hooks only after this stage has not denied the request.
    pub async fn preflight_for_turn(
        &self,
        session_id: &Id,
        turn_id: &Id,
        input: SubmitToolCall,
    ) -> Result<PreparedToolCall, ExecutionError> {
        let session = self.store.get_session(session_id).await?;
        validate_tool_arguments(&input.tool, &input.arguments)?;
        let request = ToolRequest {
            id: Id::new("tool"),
            scope: input.scope,
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool: input.tool,
            arguments: input.arguments,
            created_at: chrono::Utc::now(),
        };
        let (policy, metadata) = self
            .evaluate_policy(&request, &session.workspace_uri)
            .await?;
        Ok(PreparedToolCall {
            request,
            policy,
            metadata,
        })
    }

    pub async fn submit_prepared(
        &self,
        prepared: PreparedToolCall,
    ) -> Result<ToolCallOutcome, ExecutionError> {
        let PreparedToolCall {
            request,
            policy,
            metadata,
        } = prepared;
        match policy.decision {
            PolicyDecision::Deny => {
                let call = self
                    .store
                    .create_tool_call(request, policy, metadata, ToolCallStatus::Denied)
                    .await?;
                Ok(ToolCallOutcome::Denied { tool_call: call })
            }
            PolicyDecision::Ask => {
                let call = self
                    .store
                    .create_tool_call(request, policy, metadata, ToolCallStatus::AwaitingApproval)
                    .await?;
                let approval = self.store.create_approval(&call).await?;
                Ok(ToolCallOutcome::AwaitingApproval {
                    tool_call: call,
                    approval: Box::new(approval),
                })
            }
            PolicyDecision::Allow => {
                let call = self
                    .store
                    .create_tool_call(request, policy, metadata, ToolCallStatus::Running)
                    .await?;
                self.execute(call).await
            }
        }
    }

    pub async fn submit_prepared_with_automatic_approval(
        &self,
        prepared: PreparedToolCall,
    ) -> Result<ToolCallOutcome, ExecutionError> {
        if prepared.policy.decision != PolicyDecision::Ask {
            return self.submit_prepared(prepared).await;
        }
        let PreparedToolCall {
            request,
            policy,
            metadata,
        } = prepared;
        let call = self
            .store
            .create_tool_call(request, policy, metadata, ToolCallStatus::Running)
            .await?;
        self.store.create_automatic_approval(&call).await?;
        self.execute(call).await
    }

    async fn evaluate_policy(
        &self,
        request: &ToolRequest,
        workspace_uri: &str,
    ) -> Result<(s_code_protocol::PolicyResult, ToolPolicyMetadata), ExecutionError> {
        let local_policy = self.policy.evaluate(request);
        let mut metadata = ToolPolicyMetadata::default();
        let mut policy = if local_policy.decision == PolicyDecision::Deny {
            local_policy
        } else {
            match self
                .store
                .latest_team_configuration(&request.scope.organization_id, &request.scope.team_id)
                .await?
            {
                Some(envelope)
                    if {
                        let now = chrono::Utc::now();
                        envelope.payload.issued_at <= now && now < envelope.payload.expires_at
                    } =>
                {
                    let sequence = envelope.payload.sequence;
                    let config = envelope.payload.configuration;
                    metadata.team_configuration_sequence = Some(sequence);
                    if applies_to_rollout(
                        config.policy_rollout_percent,
                        &config.policy_rollout_seed,
                        &self.device_id,
                    ) {
                        let central = config.policy_bundle.evaluate(request);
                        if config.policy_simulate {
                            metadata.simulated_policy = Some(central);
                            local_policy
                        } else {
                            metadata.central_policy_applied = true;
                            central
                        }
                    } else {
                        local_policy
                    }
                }
                Some(_) => s_code_protocol::PolicyResult {
                    requires_approval: false,
                    decision: PolicyDecision::Deny,
                    policy_id: "central-team-configuration-validity".into(),
                    policy_version: "fail-closed".into(),
                    reason: "central Team configuration is outside its validity window".into(),
                },
                None => local_policy,
            }
        };
        if policy.decision == PolicyDecision::Ask
            && let Some(exception) = self
                .store
                .matching_policy_exception(
                    &request.scope,
                    &request.tool,
                    workspace_uri,
                    chrono::Utc::now(),
                )
                .await?
        {
            policy = s_code_protocol::PolicyResult {
                requires_approval: false,
                decision: PolicyDecision::Allow,
                policy_id: format!("exception:{}", exception.exception_id.0),
                policy_version: exception.sequence.to_string(),
                reason: format!(
                    "time-bounded exception approved by {}: {}",
                    exception.approved_by.0, exception.reason
                ),
            };
        }
        Ok((policy, metadata))
    }

    pub async fn resolve(
        &self,
        approval_id: &Id,
        input: ResolveApproval,
    ) -> Result<ToolCallOutcome, ExecutionError> {
        if input.approval_scope != s_code_protocol::ApprovalScope::Once {
            return Err(ExecutionError::Approval(
                "the Community Preview supports one-operation approvals only".into(),
            ));
        }
        let approval = self
            .store
            .resolve_approval(
                approval_id,
                &input.scope,
                input.approved,
                input.approval_scope,
            )
            .await?;
        let call = self.store.get_tool_call(&approval.tool_call_id).await?;
        if call.status != ToolCallStatus::AwaitingApproval {
            return Err(ExecutionError::Approval(
                "tool call is no longer awaiting approval".into(),
            ));
        }
        let turn = self
            .store
            .get_turn(&call.request.scope, &call.request.turn_id)
            .await?;
        let manual = turn
            .checkpoint
            .as_ref()
            .and_then(|value| value.get("execution_mode"))
            .and_then(Value::as_str)
            == Some("manual_tool");
        let turn_id = turn.id.clone();
        if approval.status == ApprovalStatus::Rejected {
            let call = self
                .store
                .finish_tool_call(&call.request.id, ToolCallStatus::Denied, None, None)
                .await?;
            if manual {
                self.store
                    .update_turn(
                        &call.request.scope,
                        &call.request.turn_id,
                        s_code_protocol::TurnStatus::Failed,
                        None,
                        Some("manual tool rejected"),
                    )
                    .await?;
            }
            return Ok(ToolCallOutcome::Denied { tool_call: call });
        }
        if manual {
            self.store
                .update_turn(
                    &call.request.scope,
                    &call.request.turn_id,
                    s_code_protocol::TurnStatus::RunningTool,
                    Some(&serde_json::json!({"execution_mode":"manual_tool"})),
                    None,
                )
                .await?;
        }
        let call = self
            .store
            .finish_tool_call(&call.request.id, ToolCallStatus::Running, None, None)
            .await?;
        let outcome = self.execute(call).await?;
        if manual {
            self.finish_manual_turn(&approval.scope, &turn_id, &outcome)
                .await?;
        }
        Ok(outcome)
    }

    async fn finish_manual_turn(
        &self,
        scope: &s_code_protocol::Scope,
        turn_id: &Id,
        outcome: &ToolCallOutcome,
    ) -> Result<(), ExecutionError> {
        match outcome {
            ToolCallOutcome::AwaitingApproval { .. } => {
                let marker = serde_json::json!({"execution_mode":"manual_tool"});
                self.store
                    .update_turn(
                        scope,
                        turn_id,
                        s_code_protocol::TurnStatus::AwaitingApproval,
                        Some(&marker),
                        None,
                    )
                    .await?;
            }
            ToolCallOutcome::Completed { .. } => {
                self.store
                    .update_turn(
                        scope,
                        turn_id,
                        s_code_protocol::TurnStatus::CallingModel,
                        None,
                        None,
                    )
                    .await?;
                self.store
                    .update_turn(
                        scope,
                        turn_id,
                        s_code_protocol::TurnStatus::Completed,
                        None,
                        None,
                    )
                    .await?;
            }
            ToolCallOutcome::Denied { .. } | ToolCallOutcome::Failed { .. } => {
                self.store
                    .update_turn(
                        scope,
                        turn_id,
                        s_code_protocol::TurnStatus::Failed,
                        None,
                        Some("manual tool did not complete"),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn undo_turn(
        &self,
        scope: &s_code_protocol::Scope,
        turn_id: &Id,
    ) -> Result<UndoTurnResult, ExecutionError> {
        let turn = self.store.get_turn(scope, turn_id).await?;
        if !matches!(
            turn.status,
            s_code_protocol::TurnStatus::Completed
                | s_code_protocol::TurnStatus::Failed
                | s_code_protocol::TurnStatus::Cancelled
        ) {
            return Err(ExecutionError::Arguments(
                "a Turn can only be undone after it reaches a terminal state".into(),
            ));
        }
        let session = self.store.get_session(&turn.session_id).await?;
        let history = self.store.list_messages(scope, &turn.session_id).await?;
        let mut previously_undone = BTreeSet::new();
        for marker in history
            .iter()
            .filter(|message| message.role == "conversation_undo_state")
        {
            if marker.content["target_turn_id"].as_str() == Some(turn_id.0.as_str()) {
                return Err(ExecutionError::Arguments(
                    "this Turn has already been removed from conversation time".into(),
                ));
            }
            previously_undone.extend(
                marker.content["omitted_message_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
        let target_message_ids = history
            .iter()
            .filter(|message| {
                message.turn_id == *turn_id
                    && matches!(message.role.as_str(), "user" | "assistant" | "tool")
                    && !previously_undone.contains(&message.id.0)
            })
            .map(|message| message.id.0.clone())
            .collect::<Vec<_>>();
        if let Some(last_target_index) = history.iter().rposition(|message| {
            target_message_ids
                .iter()
                .any(|message_id| message_id == &message.id.0)
        }) && history.iter().skip(last_target_index + 1).any(|message| {
            matches!(message.role.as_str(), "user" | "assistant" | "tool")
                && !previously_undone.contains(&message.id.0)
        }) {
            return Err(ExecutionError::Arguments(
                "only the latest conversation Turn can be undone".into(),
            ));
        }
        let source_message_count = u64::try_from(history.len()).unwrap_or(u64::MAX);
        let source_latest_message_id = history.last().map(|message| message.id.clone());
        let plan_item_ids = self
            .store
            .list_session_events(scope, &turn.session_id, 10_000)
            .await?
            .into_iter()
            .filter(|event| event.turn_id.as_ref() == Some(turn_id) && event.kind == "plan.updated")
            .filter_map(|event| {
                event
                    .payload
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        let session_goal_rewound = self
            .store
            .session_goal_checkpoint_is_restorable(scope, &turn.session_id, turn_id)
            .await?;
        let runtime = ToolRuntime::open(&session.workspace_uri, self.platform.clone())?;
        let changes = self.store.list_turn_file_changes(scope, turn_id).await?;
        let mut effective_after = Vec::with_capacity(changes.len());
        for change in &changes {
            let current = runtime.snapshot_file(&change.path)?;
            let is_before = current.sha256 == change.before_sha256;
            let is_after = current.sha256.as_deref() == Some(&change.after_sha256);
            let is_previous = change
                .previous_after_sha256
                .as_ref()
                .is_some_and(|previous| current.sha256.as_ref() == Some(previous));
            if !is_before && !is_after && !is_previous {
                return Err(ToolError::ConcurrentModification.into());
            }
            effective_after.push(if is_before {
                None
            } else if is_after {
                Some(change.after_sha256.clone())
            } else {
                change.previous_after_sha256.clone()
            });
        }
        let cancelled_inputs = self
            .store
            .cancel_turn_inputs_for_undo(scope, turn_id)
            .await?;
        let mut restored_paths = Vec::with_capacity(changes.len());
        for (change, effective_after) in changes.into_iter().zip(effective_after) {
            if let Some(expected_after) = effective_after {
                runtime.restore_file(
                    &change.path,
                    &expected_after,
                    change.before_content.as_deref(),
                )?;
            }
            self.store
                .mark_turn_file_change_undone(scope, turn_id, &change.path)
                .await?;
            restored_paths.push(change.path);
        }
        restored_paths.sort();
        if session_goal_rewound {
            self.store
                .restore_session_goal_checkpoint(scope, &turn.session_id, turn_id)
                .await?;
        }
        self.store
            .append_conversation_undo(
                scope,
                &turn.session_id,
                turn_id,
                source_message_count,
                source_latest_message_id.as_ref(),
                serde_json::json!({
                    "schema_version": 1,
                    "target_turn_id": turn_id.0,
                    "omitted_message_ids": target_message_ids,
                    "rewound_plan_item_ids": plan_item_ids,
                    "cancelled_input_ids": cancelled_inputs
                        .iter()
                        .map(|input| input.id.0.as_str())
                        .collect::<Vec<_>>(),
                    "source_message_count": source_message_count,
                    "source_latest_message_id": source_latest_message_id
                        .as_ref()
                        .map(|id| id.0.as_str()),
                }),
            )
            .await?;
        Ok(UndoTurnResult {
            turn_id: turn_id.clone(),
            restored_paths,
            conversation_omitted_messages: u32::try_from(target_message_ids.len())
                .unwrap_or(u32::MAX),
            plan_items_rewound: u32::try_from(plan_item_ids.len()).unwrap_or(u32::MAX),
            cancelled_input_ids: cancelled_inputs.into_iter().map(|input| input.id).collect(),
            session_goal_rewound,
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolCallOutcome, ExecutionError> {
        let result = self.dispatch(&call).await;
        match result {
            Ok(value) => {
                let value = s_code_audit::redact(value);
                let call = self
                    .store
                    .finish_tool_call(
                        &call.request.id,
                        ToolCallStatus::Completed,
                        Some(&value),
                        None,
                    )
                    .await?;
                Ok(ToolCallOutcome::Completed { tool_call: call })
            }
            Err(error) => {
                let message = s_code_audit::redact_text(&error.to_string());
                let call = self
                    .store
                    .finish_tool_call(
                        &call.request.id,
                        ToolCallStatus::Failed,
                        None,
                        Some(&message),
                    )
                    .await?;
                Ok(ToolCallOutcome::Failed { tool_call: call })
            }
        }
    }

    async fn dispatch(&self, call: &ToolCall) -> Result<Value, ExecutionError> {
        let session = self.store.get_session(&call.request.session_id).await?;
        let runtime = ToolRuntime::open(&session.workspace_uri, self.platform.clone())?;
        match call.request.tool.as_str() {
            "list_files" => {
                let args: ListArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(runtime.list_files(
                    args.path.as_deref().unwrap_or("."),
                    args.depth.unwrap_or(3).min(32),
                    args.limit.unwrap_or(2000).min(10000),
                )?)
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "read_file" => {
                let args: ReadArgs = args(&call.request.arguments)?;
                let start_line = args.start_line.unwrap_or(1);
                let value = serde_json::to_value(runtime.read_file(
                    &args.path,
                    start_line,
                    args.end_line.unwrap_or(2000),
                    bounded_tool_output_bytes(args.max_bytes, 128 * 1024),
                )?)
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?;
                Ok(number_file_content(value, start_line))
            }
            "search_text" => {
                let args: SearchArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    runtime
                        .search_text(
                            &args.query,
                            args.glob.as_deref(),
                            bounded_tool_output_bytes(args.max_bytes, 256 * 1024),
                        )
                        .await?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "apply_patch" => self.apply_edits(call, &runtime).await,
            "run_command" => {
                let args: RunArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    runtime
                        .run_with_compatibility(
                            &args.program,
                            args.args,
                            Duration::from_secs(args.timeout_seconds.unwrap_or(60).min(600)),
                            args.network_enabled.unwrap_or(false),
                            bounded_tool_output_bytes(args.max_bytes, 1024 * 1024),
                            CommandCompatibility {
                                workspace_writable: args.sandbox_profile.workspace_writable(),
                                browser_compatible: args.sandbox_profile.browser_compatible(),
                            },
                        )
                        .await?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "git_status" => Ok(serde_json::to_value(
                GitService::open(&session.workspace_uri)
                    .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                    .status()
                    .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
            )
            .map_err(|error| ExecutionError::Arguments(error.to_string()))?),
            "git_diff" => {
                let args: GitDiffArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    GitService::open(&session.workspace_uri)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                        .diff_revision(
                            args.revision.as_deref(),
                            &args.paths,
                            bounded_tool_output_bytes(args.max_bytes, 1024 * 1024),
                        )
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "git_suggest_reviewers" => {
                let args: GitReviewerArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    GitService::open(&session.workspace_uri)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                        .suggest_reviewers(&args.paths, &args.team_reviewers)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "git_create_branch" => {
                let args: GitBranchArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    GitService::open(&session.workspace_uri)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                        .create_worktree(&args.base, &args.branch)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "git_commit" => {
                let args: GitCommitArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    GitService::open(&session.workspace_uri)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                        .commit(CommitRequest {
                            message: args.message,
                            paths: args.paths,
                            expected_diff_hash: args.expected_diff_hash,
                            scope: call.request.scope.clone(),
                            session_id: call.request.session_id.clone(),
                        })
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "git_push" => Err(ExecutionError::Arguments(
                "automatic git push is unavailable in the Community Preview; inspect the commit and push it with your Git client".into(),
            )),
            name => match &self.external {
                Some(external) => external
                    .execute(call)
                    .await
                    .map_err(ExecutionError::Arguments)?
                    .ok_or_else(|| {
                        ExecutionError::Arguments(format!("tool {name} has no registered executor"))
                    }),
                None => Err(ExecutionError::Arguments(format!(
                    "tool {name} has no registered executor"
                ))),
            },
        }
    }
}

fn args<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, ExecutionError> {
    serde_json::from_value(value.clone())
        .map_err(|error| ExecutionError::Arguments(error.to_string()))
}

fn validate_tool_arguments(tool: &str, value: &Value) -> Result<(), ExecutionError> {
    if !value.is_object() {
        return Err(ExecutionError::Arguments(
            "tool arguments must be a JSON object".into(),
        ));
    }
    match tool {
        "list_files" => {
            let _: ListArgs = args(value)?;
        }
        "read_file" => {
            let _: ReadArgs = args(value)?;
        }
        "search_text" => {
            let _: SearchArgs = args(value)?;
        }
        "apply_patch" => {
            editing::parse(value)?;
        }
        "run_command" => {
            let _: RunArgs = args(value)?;
        }
        "git_status" => {}
        "git_diff" => {
            let _: GitDiffArgs = args(value)?;
        }
        "git_suggest_reviewers" => {
            let _: GitReviewerArgs = args(value)?;
        }
        "git_create_branch" => {
            let _: GitBranchArgs = args(value)?;
        }
        "git_commit" => {
            let _: GitCommitArgs = args(value)?;
        }
        "git_push" => {
            return Err(ExecutionError::Arguments(
                "automatic git push is unavailable in the Community Preview".into(),
            ));
        }
        "tool_search" => {
            let input: ToolSearchValidation = args(value)?;
            if input.query.trim().is_empty()
                || input.query.chars().count() > 200
                || input.limit.is_some_and(|limit| !(1..=20).contains(&limit))
            {
                return Err(ExecutionError::Arguments(
                    "tool_search requires a 1 to 200 character query and limit from 1 to 20".into(),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Deserialize)]
struct ToolSearchValidation {
    query: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct ListArgs {
    path: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SearchArgs {
    query: String,
    glob: Option<String>,
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct RunArgs {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    timeout_seconds: Option<u64>,
    network_enabled: Option<bool>,
    max_bytes: Option<usize>,
    #[serde(default)]
    sandbox_profile: RunSandboxProfile,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RunSandboxProfile {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    BrowserTest,
}

impl RunSandboxProfile {
    fn workspace_writable(self) -> bool {
        matches!(self, Self::WorkspaceWrite | Self::BrowserTest)
    }

    fn browser_compatible(self) -> bool {
        matches!(self, Self::BrowserTest)
    }
}

#[derive(Deserialize)]
struct GitDiffArgs {
    #[serde(default)]
    paths: Vec<String>,
    revision: Option<String>,
    max_bytes: Option<usize>,
}

#[derive(Deserialize)]
struct GitReviewerArgs {
    paths: Vec<String>,
    #[serde(default)]
    team_reviewers: Vec<String>,
}

#[derive(Deserialize)]
struct GitBranchArgs {
    base: String,
    branch: String,
}

#[derive(Deserialize)]
struct GitCommitArgs {
    message: String,
    paths: Vec<String>,
    expected_diff_hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use s_code_platform_runtime::{ProcessOutput, ProcessSpec, RuntimeError};
    use s_code_protocol::{
        ApprovalScope, CreateSession, CreateTurnInput, Scope, SessionGoalStatus, SetSessionGoal,
        TurnInputMode, TurnInputStatus, UpdateSessionGoal,
    };
    use s_code_tool_runtime::content_sha256;
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct FakeRuntime;
    struct SecretOutputRuntime;

    struct FakeExternal {
        calls: Arc<AtomicUsize>,
    }

    #[test]
    fn exact_text_edits_apply_in_order_without_resending_the_file() {
        let patch: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "edits": [
                {"old_text": "one", "new_text": "ONE"},
                {"old_text": "three", "new_text": "THREE"}
            ]
        }))
        .unwrap();
        let replacement = patch.into_replacement(b"one\ntwo\nthree\n").unwrap();
        assert_eq!(replacement.content, "ONE\ntwo\nTHREE\n");
        assert_eq!(replacement.expected_sha256.as_deref(), Some("abc"));
    }

    #[test]
    fn short_file_revisions_preserve_optimistic_concurrency_checks() {
        let current = content_sha256(b"hello");
        assert!(expected_revision_matches(&current, &current));
        assert!(expected_revision_matches(&current, &current[..16]));
        assert!(!expected_revision_matches(&current, &current[..15]));
        assert!(!expected_revision_matches(&current, "0000000000000000"));
    }

    #[test]
    fn read_results_expose_absolute_line_numbers_without_duplicating_content() {
        let value = number_file_content(
            json!({"path":"src/lib.rs","content":"second\nthird","revision":"abc"}),
            2,
        );
        assert_eq!(value["numbered_content"], "2: second\n3: third");
        assert!(value.get("content").is_none());
        assert!(
            value["content_format"]
                .as_str()
                .unwrap()
                .contains("metadata")
        );
    }

    #[test]
    fn line_range_edits_apply_bottom_up_and_preserve_line_boundaries() {
        let patch: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "edits": [
                {"start_line": 2, "end_line": 2, "new_text": "TWO"},
                {"start_line": 4, "end_line": 4, "new_text": "FOUR\nand more"}
            ]
        }))
        .unwrap();
        let replacement = patch.into_replacement(b"one\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(replacement.content, "one\nTWO\nthree\nFOUR\nand more\n");
    }

    #[test]
    fn line_range_edits_reject_invalid_overlapping_or_mixed_modes() {
        let overlapping: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "edits": [
                {"start_line": 1, "end_line": 2, "new_text": "first"},
                {"start_line": 2, "end_line": 3, "new_text": "second"}
            ]
        }))
        .unwrap();
        assert!(matches!(
            overlapping.into_replacement(b"one\ntwo\nthree\n"),
            Err(ExecutionError::Arguments(message)) if message.contains("must not overlap")
        ));

        let mixed: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "edits": [
                {"old_text": "one", "new_text": "ONE"},
                {"start_line": 2, "end_line": 2, "new_text": "TWO"}
            ]
        }))
        .unwrap();
        assert!(matches!(
            mixed.into_replacement(b"one\ntwo\n"),
            Err(ExecutionError::Arguments(message)) if message.contains("one mode consistently")
        ));
    }

    #[test]
    fn exact_text_edits_reject_ambiguous_matches_and_mixed_modes() {
        let ambiguous: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "edits": [{"old_text": "same", "new_text": "changed"}]
        }))
        .unwrap();
        assert!(matches!(
            ambiguous.into_replacement(b"same same"),
            Err(ExecutionError::Arguments(message)) if message.contains("matched 2 times")
        ));

        let mixed: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "src/lib.rs",
            "expected_sha256": "abc",
            "content": "replacement",
            "edits": [{"old_text": "old", "new_text": "new"}]
        }))
        .unwrap();
        assert!(mixed.validate_shape().is_err());
    }

    #[test]
    fn exact_text_mismatch_returns_a_bounded_nearby_recovery_excerpt() {
        let patch: ApplyPatchArgs = serde_json::from_value(json!({
            "path": "script.py",
            "expected_revision": "1234567890abcdef",
            "edits": [{
                "old_text": "import json, os, subprocess, sys, tempfile",
                "new_text": "replacement"
            }]
        }))
        .unwrap();
        let error = patch
            .into_replacement(
                b"import json\nimport os\nimport subprocess\nimport sys\nimport tempfile\n",
            )
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Do not retry the same old_text"));
        assert!(message.contains("import json\nimport os\nimport subprocess"));
    }

    #[test]
    fn run_command_profiles_are_explicit_and_fail_closed() {
        let read_only: RunArgs = serde_json::from_value(json!({
            "program": "cargo",
            "sandbox_profile": "read-only"
        }))
        .unwrap();
        assert!(!read_only.sandbox_profile.workspace_writable());

        let default_profile: RunArgs = serde_json::from_value(json!({"program": "cargo"})).unwrap();
        assert!(default_profile.sandbox_profile.workspace_writable());
        assert!(!default_profile.sandbox_profile.browser_compatible());

        let browser_profile: RunArgs = serde_json::from_value(json!({
            "program": "npm",
            "sandbox_profile": "browser-test"
        }))
        .unwrap();
        assert!(browser_profile.sandbox_profile.workspace_writable());
        assert!(browser_profile.sandbox_profile.browser_compatible());

        assert!(
            serde_json::from_value::<RunArgs>(json!({
                "program": "cargo",
                "sandbox_profile": "host-unrestricted"
            }))
            .is_err()
        );
    }

    #[test]
    fn tool_output_limits_are_clamped_at_the_execution_boundary() {
        assert_eq!(
            bounded_tool_output_bytes(Some(usize::MAX), 128 * 1024),
            MAX_TOOL_OUTPUT_BYTES
        );
        assert_eq!(bounded_tool_output_bytes(Some(0), 128 * 1024), 1);
        assert_eq!(bounded_tool_output_bytes(None, 128 * 1024), 128 * 1024);
    }

    #[async_trait]
    impl ExternalToolExecutor for FakeExternal {
        async fn execute(&self, call: &ToolCall) -> Result<Option<Value>, String> {
            if call.request.tool == "scm_create_draft_pr" {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Some(json!({"draft":true,"url":"https://example/pr/1"})))
            } else {
                Ok(None)
            }
        }
    }

    #[async_trait]
    impl PlatformRuntime for FakeRuntime {
        fn capabilities(&self) -> Vec<s_code_protocol::Capability> {
            vec![]
        }
        fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError> {
            url::Url::parse(uri)
                .unwrap()
                .to_file_path()
                .unwrap()
                .canonicalize()
                .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))
        }
        async fn execute(&self, _: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
            Ok(ProcessOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            })
        }
        async fn cancel_process_tree(&self, _: &str) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    #[async_trait]
    impl PlatformRuntime for SecretOutputRuntime {
        fn capabilities(&self) -> Vec<s_code_protocol::Capability> {
            vec![]
        }
        fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError> {
            url::Url::parse(uri)
                .unwrap()
                .to_file_path()
                .unwrap()
                .canonicalize()
                .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))
        }
        async fn execute(&self, _: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
            Ok(ProcessOutput {
                exit_code: Some(0),
                stdout: "OPENAI_API_KEY=sk-project-1234567890\n".into(),
                stderr: "Authorization: Bearer model-must-not-see-this".into(),
                truncated: false,
            })
        }
        async fn cancel_process_tree(&self, _: &str) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    fn scope(team: &str) -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id(team.into()),
            actor_id: Id("user".into()),
            goal_id: None,
            task_id: None,
        }
    }

    async fn service() -> (tempfile::TempDir, ExecutionService, Id) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team"),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "test".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        (
            dir,
            ExecutionService::new(store, PolicyBundle::default(), Arc::new(FakeRuntime)),
            session.id,
        )
    }

    async fn approve_edit(
        service: &ExecutionService,
        session: &Id,
        arguments: Value,
    ) -> ToolCallOutcome {
        let outcome = service
            .submit(
                session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments,
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::AwaitingApproval { approval, .. } = outcome else {
            panic!("approval expected: {outcome:?}")
        };
        service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn all_editing_formats_modify_multiple_files_and_undo_together() {
        let revision = content_sha256(b"hello\n")[..16].to_string();
        for arguments in [
            json!({"files":[{"path":"a.txt","expected_revision":revision,"edits":[{"start_line":1,"end_line":1,"new_text":"updated\n"}]},{"path":"new.txt","expected_revision":null,"content":"new\n"}]}),
            json!({"files":[{"path":"a.txt","expected_revision":revision,"edits":[{"old_text":"hello","new_text":"updated"}]},{"path":"new.txt","expected_revision":null,"content":"new\n"}]}),
            json!({"patch":"*** Begin Patch\n*** Update File: a.txt\n@@\n-hello\n+updated\n*** Add File: new.txt\n+new\n*** End Patch","revisions":{"a.txt":revision,"new.txt":null}}),
        ] {
            let (dir, service, session) = service().await;
            fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
            let outcome = approve_edit(&service, &session, arguments).await;
            let ToolCallOutcome::Completed { tool_call } = outcome else {
                panic!("{outcome:?}")
            };
            assert_eq!(
                tool_call.result.unwrap()["files"].as_array().unwrap().len(),
                2
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "updated\n"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("new.txt")).unwrap(),
                "new\n"
            );
            let undone = service
                .undo_turn(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(undone.restored_paths.len(), 2);
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello\n"
            );
            assert!(!dir.path().join("new.txt").exists());
        }
    }

    #[tokio::test]
    async fn batch_prevalidates_stale_denied_and_invalid_second_targets() {
        for second in [
            json!({"path":"a.txt","expected_revision":"0000000000000000","content":"bad"}),
            json!({"path":".env","expected_revision":null,"content":"bad"}),
            json!({"path":"a.txt","expected_revision":content_sha256(b"hello"),"edits":[{"old_text":"missing","new_text":"bad"}]}),
        ] {
            let (dir, service, session) = service().await;
            let outcome = approve_edit(&service, &session, json!({"files":[{"path":"first.txt","expected_revision":null,"content":"first"},second]})).await;
            assert!(
                matches!(outcome, ToolCallOutcome::Failed { .. }),
                "{outcome:?}"
            );
            assert!(!dir.path().join("first.txt").exists());
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello"
            );
            assert!(!dir.path().join(".env").exists());
        }
    }

    #[tokio::test]
    async fn batch_reports_partial_failure_and_keeps_undo_records() {
        let (dir, service, session) = service().await;
        let outcome = service.submit(&session, SubmitToolCall {scope:scope("team"),tool:"apply_patch".into(),arguments:json!({"files":[{"path":"first.txt","expected_revision":null,"content":"first"},{"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"updated"}]})}).await.unwrap();
        let ToolCallOutcome::AwaitingApproval {
            approval,
            tool_call,
        } = outcome
        else {
            panic!("approval expected")
        };
        // A conflicting pending storage operation causes a deterministic failure
        // after the first write, independently of filesystem privileges.
        let plan = service
            .store
            .plan_turn_file_change(
                &scope("team"),
                &tool_call.request.turn_id,
                "a.txt",
                Some(b"hello"),
                Some(&content_sha256(b"hello")),
                &content_sha256(b"other"),
            )
            .await
            .unwrap();
        let outcome = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("partial failure expected")
        };
        let error = tool_call.error.unwrap();
        assert!(error.contains("applied files: [\"first.txt\"]"), "{error}");
        assert_eq!(
            fs::read_to_string(dir.path().join("first.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello"
        );
        service.store.abort_turn_file_change(&plan).await.unwrap();
        service
            .undo_turn(&scope("team"), &tool_call.request.turn_id)
            .await
            .unwrap();
        assert!(!dir.path().join("first.txt").exists());
    }

    #[test]
    fn exact_text_rejects_overlapping_utf8_matches() {
        let patch: ApplyPatchArgs = serde_json::from_value(
            json!({"path":"a","edits":[{"old_text":"éé","new_text":"replacement"}]}),
        )
        .unwrap();
        let error = patch.into_replacement("ééé".as_bytes()).unwrap_err();
        assert!(error.to_string().contains("matched 2 times"));
    }

    #[tokio::test]
    async fn ambiguous_patch_batch_leaves_every_file_unchanged() {
        let (dir, service, session) = service().await;
        fs::write(dir.path().join("a.txt"), "foo\nfoo\nfoo\n").unwrap();
        let outcome = approve_edit(&service, &session, json!({"patch":"*** Begin Patch\n*** Add File: new.txt\n+new\n*** Update File: a.txt\n@@\n-foo\n-foo\n+replacement\n*** End Patch", "revisions":{"a.txt":content_sha256(b"foo\nfoo\nfoo\n"),"new.txt":null}})).await;
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("ambiguous patch must fail")
        };
        assert!(tool_call.error.unwrap().contains("matched 2 times"));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "foo\nfoo\nfoo\n"
        );
        assert!(!dir.path().join("new.txt").exists());
    }

    #[tokio::test]
    async fn batch_handles_case_aliases_without_breaking_undo() {
        let (dir, service, session) = service().await;
        let aliases = dir.path().join("A.txt").exists();
        if !aliases {
            fs::write(dir.path().join("A.txt"), "hello").unwrap();
        }
        let outcome = approve_edit(
            &service,
            &session,
            json!({"files":[
                {"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"first"},
                {"path":"A.txt","expected_revision":content_sha256(b"hello"),"content":"second"}
            ]}),
        )
        .await;
        let call = if aliases {
            let ToolCallOutcome::Failed { tool_call } = outcome else {
                panic!("same-file aliases must fail")
            };
            assert!(
                tool_call
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("duplicate file target")
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello"
            );
            assert!(
                service
                    .store
                    .list_turn_file_changes(&scope("team"), &tool_call.request.turn_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
            tool_call
        } else {
            let ToolCallOutcome::Completed { tool_call } = outcome else {
                panic!("distinct files should be editable")
            };
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "first"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("A.txt")).unwrap(),
                "second"
            );
            tool_call
        };
        service
            .undo_turn(&scope("team"), &call.request.turn_id)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("A.txt")).unwrap(),
            "hello"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_rejects_hard_link_aliases_before_writing() {
        let (dir, service, session) = service().await;
        fs::hard_link(dir.path().join("a.txt"), dir.path().join("linked.txt")).unwrap();
        let outcome = approve_edit(&service, &session, json!({"files":[
            {"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"first"},
            {"path":"linked.txt","expected_revision":content_sha256(b"hello"),"content":"second"}
        ]})).await;
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("same-file aliases must fail")
        };
        assert!(tool_call.error.unwrap().contains("duplicate file target"));
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("linked.txt")).unwrap(),
            "hello"
        );
        service
            .undo_turn(&scope("team"), &tool_call.request.turn_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn batch_rejects_case_colliding_new_names_before_creating_files() {
        let (dir, service, session) = service().await;
        let outcome = approve_edit(
            &service,
            &session,
            json!({"files":[
                {"path":"new.txt","expected_revision":null,"content":"first"},
                {"path":"NEW.txt","expected_revision":null,"content":"second"}
            ]}),
        )
        .await;
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("case-colliding new names must fail")
        };
        assert!(tool_call.error.unwrap().contains("duplicate file target"));
        assert!(!dir.path().join("new.txt").exists());
        assert!(!dir.path().join("NEW.txt").exists());
    }

    #[tokio::test]
    async fn stale_write_does_not_claim_external_changes_or_poison_other_undo() {
        for external_content in ["external change", "desired"] {
            let (dir, service, session) = service().await;
            let outcome = approve_edit(&service, &session, json!({"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"first"})).await;
            let ToolCallOutcome::Completed { tool_call } = outcome else {
                panic!("first edit must succeed")
            };
            let uri = url::Url::from_directory_path(dir.path())
                .unwrap()
                .to_string();
            let runtime = ToolRuntime::open(&uri, Arc::new(FakeRuntime)).unwrap();
            fs::write(dir.path().join("b.txt"), "before").unwrap();
            let snapshot = runtime.snapshot_file("b.txt").unwrap();
            let replacement = FileReplacement {
                path: "b.txt".into(),
                expected_sha256: snapshot.sha256.clone(),
                content: "desired".into(),
            };
            // Deterministic external change between preparation and the actual
            // write, including one whose content equals the desired result.
            fs::write(dir.path().join("b.txt"), external_content).unwrap();
            let mut results = Vec::new();
            let error = service
                .write_prepared_edit(
                    &tool_call,
                    &runtime,
                    (snapshot, replacement, content_sha256(b"desired")),
                    &mut results,
                )
                .await
                .unwrap_err();
            assert!(matches!(
                error,
                ExecutionError::Tool(ToolError::ConcurrentModification)
            ));
            assert!(
                results.is_empty(),
                "a rejected write must not be reported as applied"
            );
            let changes = service
                .store
                .list_turn_file_changes(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].path, "a.txt");
            service
                .undo_turn(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("b.txt")).unwrap(),
                external_content
            );
        }
    }

    #[tokio::test]
    async fn io_failure_does_not_claim_external_writes_or_poison_other_undo() {
        for existing in [false, true] {
            let (dir, service, session) = service().await;
            let outcome = approve_edit(&service, &session, json!({"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"first"})).await;
            let ToolCallOutcome::Completed { tool_call } = outcome else {
                panic!("first write expected")
            };
            let uri = url::Url::from_directory_path(dir.path())
                .unwrap()
                .to_string();
            let runtime = ToolRuntime::open(&uri, Arc::new(FakeRuntime)).unwrap();
            if existing {
                fs::write(dir.path().join("b.txt"), "before").unwrap();
            }
            let snapshot = runtime.snapshot_file("b.txt").unwrap();
            let replacement = FileReplacement {
                path: "b.txt".into(),
                expected_sha256: snapshot.sha256.clone(),
                content: "after".into(),
            };
            let mut results = Vec::new();
            let error = service
                .write_prepared_edit_with(
                    &tool_call,
                    (snapshot, replacement, content_sha256(b"after")),
                    &mut results,
                    |_| {
                        // Another editor writes the requested bytes while our write fails BEFORE rename.
                        fs::write(dir.path().join("b.txt"), "after").unwrap();
                        Err(ToolError::Io(std::io::Error::other(
                            "injected pre-rename failure",
                        )))
                    },
                )
                .await
                .unwrap_err();
            assert!(matches!(error, ExecutionError::Tool(ToolError::Io(_))));
            assert!(
                results.is_empty(),
                "a failed write must not be reported as applied"
            );
            let changes = service
                .store
                .list_turn_file_changes(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].path, "a.txt");
            service
                .undo_turn(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello"
            );
            assert_eq!(
                runtime.snapshot_file("b.txt").unwrap().content,
                Some(b"after".to_vec()),
                "Undo must preserve the external write (existing={existing}, reported={results:?})"
            );
        }
    }

    #[tokio::test]
    async fn post_rename_failure_preserves_applied_paths_and_turn_undo() {
        for existing in [false, true] {
            let (dir, service, session) = service().await;
            let outcome = approve_edit(&service, &session, json!({"path":"a.txt","expected_revision":content_sha256(b"hello"),"content":"first"})).await;
            let ToolCallOutcome::Completed { tool_call } = outcome else {
                panic!("first write expected")
            };
            let uri = url::Url::from_directory_path(dir.path())
                .unwrap()
                .to_string();
            let runtime = ToolRuntime::open(&uri, Arc::new(FakeRuntime)).unwrap();
            if existing {
                fs::write(dir.path().join("b.txt"), "before").unwrap();
            }
            let snapshot = runtime.snapshot_file("b.txt").unwrap();
            let replacement = FileReplacement {
                path: "b.txt".into(),
                expected_sha256: snapshot.sha256.clone(),
                content: "after".into(),
            };
            let mut results = Vec::new();
            let error = service
                .write_prepared_edit_with(
                    &tool_call,
                    (snapshot, replacement, content_sha256(b"after")),
                    &mut results,
                    |replacement| {
                        // Exercise the journal path with a real completed write and the
                        // exact error returned by the runtime's post-rename sync test.
                        runtime.apply_replacement(replacement)?;
                        Err(ToolError::Durability {
                            path: "b.txt".into(),
                            source: std::io::Error::other("injected sync failure"),
                        })
                    },
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("was written"));
            assert!(error.to_string().contains("durability is unconfirmed"));
            assert_eq!(results[0]["path"], "b.txt");
            assert_eq!(results[0]["durability"], "unconfirmed");
            let changes = service
                .store
                .list_turn_file_changes(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(changes.len(), 2);
            assert!(changes.iter().all(|change| change.state == "applied"));
            service
                .undo_turn(&scope("team"), &tool_call.request.turn_id)
                .await
                .unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("a.txt")).unwrap(),
                "hello"
            );
            assert_eq!(
                runtime.snapshot_file("b.txt").unwrap().content,
                existing.then(|| b"before".to_vec())
            );
        }
    }

    #[tokio::test]
    async fn reads_execute_without_approval() {
        let (_dir, service, session) = service().await;
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "read_file".into(),
                    arguments: json!({"path":"a.txt"}),
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ToolCallOutcome::Completed { .. }));
    }

    #[tokio::test]
    async fn git_diff_never_delivers_sensitive_file_content() {
        let (dir, service, session) = service().await;
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "initial",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        std::fs::write(
            dir.path().join(".env"),
            "PASSWORD=synthetic-sensitive-value\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("public.txt"), "visible-change\n").unwrap();
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "git_diff".into(),
                    arguments: json!({"paths": ["."]}),
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Completed { tool_call } = outcome else {
            panic!("read-only diff should complete without approval")
        };
        let encoded = serde_json::to_string(&tool_call.result).unwrap();
        assert!(encoded.contains("visible-change"));
        assert!(!encoded.contains("synthetic-sensitive-value"));
        assert!(!encoded.contains(".env"));
    }

    #[tokio::test]
    async fn command_output_is_redacted_before_storage_or_model_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team"),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "secret output".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let service = ExecutionService::new(
            store,
            PolicyBundle::default(),
            Arc::new(SecretOutputRuntime),
        );
        let pending = service
            .submit(
                &session.id,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "run_command".into(),
                    arguments: json!({"program":"cargo","args":["test"]}),
                },
            )
            .await
            .unwrap();
        let approval = match pending {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            other => panic!("approval expected, got {other:?}"),
        };
        let completed = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Completed { tool_call } = completed else {
            panic!("completion expected")
        };
        let encoded = serde_json::to_string(&tool_call.result).unwrap();
        assert!(!encoded.contains("sk-project-1234567890"));
        assert!(!encoded.contains("model-must-not-see-this"));
        assert!(encoded.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn session_wide_approval_is_rejected_without_consuming_the_request() {
        let (_dir, service, session) = service().await;
        let pending = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "run_command".into(),
                    arguments: json!({"program":"cargo","args":["test"]}),
                },
            )
            .await
            .unwrap();
        let approval = match pending {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            other => panic!("approval expected, got {other:?}"),
        };
        let error = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Session,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("one-operation approvals only"));

        let completed = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        assert!(matches!(completed, ToolCallOutcome::Completed { .. }));
    }

    #[tokio::test]
    async fn preflight_validates_and_does_not_create_a_cold_approval() {
        let (_dir, service, session_id) = service().await;
        let turn = service
            .store
            .create_turn(&scope("team"), &session_id)
            .await
            .unwrap();
        let prepared = service
            .preflight_for_turn(
                &session_id,
                &turn.id,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({
                        "path":"new.txt",
                        "expected_sha256":null,
                        "content":"new"
                    }),
                },
            )
            .await
            .unwrap();

        assert_eq!(prepared.decision(), &PolicyDecision::Ask);
        assert!(
            service
                .store
                .list_session_approvals(&scope("team"), &session_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(matches!(
            service.submit_prepared(prepared).await.unwrap(),
            ToolCallOutcome::AwaitingApproval { .. }
        ));

        let invalid = service
            .preflight_for_turn(
                &session_id,
                &turn.id,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({"path":"missing-content.txt"}),
                },
            )
            .await;
        assert!(matches!(invalid, Err(ExecutionError::Arguments(_))));
    }

    #[tokio::test]
    async fn writes_require_same_team_approval() {
        let (dir, service, session) = service().await;
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({"path":"new.txt","expected_sha256":null,"content":"new"}),
                },
            )
            .await
            .unwrap();
        let approval = match outcome {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        assert!(matches!(
            service
                .resolve(
                    &approval.id,
                    ResolveApproval {
                        scope: scope("other"),
                        approved: true,
                        approval_scope: ApprovalScope::Once
                    }
                )
                .await,
            Err(ExecutionError::Storage(StorageError::ScopeMismatch))
        ));
        let outcome = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ToolCallOutcome::Completed { .. }));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "new"
        );
    }

    #[tokio::test]
    async fn stale_patch_failure_returns_the_current_short_revision() {
        let (_dir, service, session) = service().await;
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({
                        "path":"a.txt",
                        "expected_sha256":"one-character-copy-error",
                        "content":"changed"
                    }),
                },
            )
            .await
            .unwrap();
        let approval = match outcome {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let outcome = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("hash mismatch must fail without replacing the file")
        };
        let error = tool_call.error.unwrap();
        assert!(error.contains(&content_sha256(b"hello")[..16]));
    }

    #[tokio::test]
    async fn concurrent_approval_decisions_execute_a_write_once() {
        let (dir, service, session) = service().await;
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({
                        "path":"race.txt",
                        "expected_sha256":null,
                        "content":"single winner"
                    }),
                },
            )
            .await
            .unwrap();
        let approval = match outcome {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let first = service.clone();
        let second = service.clone();
        let first_approval_id = approval.id.clone();
        let second_approval_id = approval.id.clone();
        let decision = || ResolveApproval {
            scope: scope("team"),
            approved: true,
            approval_scope: ApprovalScope::Once,
        };

        let (first_result, second_result) = tokio::join!(
            first.resolve(&first_approval_id, decision()),
            second.resolve(&second_approval_id, decision())
        );
        let results = [first_result, second_result];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(ExecutionError::Storage(StorageError::InvalidState(_)))
                ))
                .count(),
            1
        );
        assert!(
            results
                .iter()
                .any(|result| matches!(result, Ok(ToolCallOutcome::Completed { .. })))
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("race.txt")).unwrap(),
            "single winner"
        );
    }

    #[tokio::test]
    async fn approved_turn_write_restores_before_image_and_created_file() {
        let (dir, service, session) = service().await;
        let existing = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({
                        "path":"a.txt",
                        "expected_sha256":content_sha256(b"hello"),
                        "content":"changed"
                    }),
                },
            )
            .await
            .unwrap();
        let approval = match existing {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let completed = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let turn_id = match completed {
            ToolCallOutcome::Completed { tool_call } => tool_call.request.turn_id,
            _ => panic!("completion expected"),
        };
        let changed_hash = content_sha256(b"changed");
        let planned_hash = content_sha256(b"planned but not written");
        service
            .store
            .plan_turn_file_change(
                &scope("team"),
                &turn_id,
                "a.txt",
                Some(b"changed"),
                Some(&changed_hash),
                &planned_hash,
            )
            .await
            .unwrap();
        let undone = service.undo_turn(&scope("team"), &turn_id).await.unwrap();
        assert_eq!(undone.restored_paths, vec!["a.txt"]);
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello"
        );
        assert!(service.undo_turn(&scope("other"), &turn_id).await.is_err());

        let created = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({"path":"created.txt","expected_sha256":null,"content":"new"}),
                },
            )
            .await
            .unwrap();
        let approval = match created {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let completed = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let turn_id = match completed {
            ToolCallOutcome::Completed { tool_call } => tool_call.request.turn_id,
            _ => panic!("completion expected"),
        };
        service.undo_turn(&scope("team"), &turn_id).await.unwrap();
        assert!(!dir.path().join("created.txt").exists());
    }

    #[tokio::test]
    async fn undo_rejects_user_changes_made_after_the_turn() {
        let (dir, service, session) = service().await;
        let proposed = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "apply_patch".into(),
                    arguments: json!({"path":"new.txt","expected_sha256":null,"content":"agent"}),
                },
            )
            .await
            .unwrap();
        let approval = match proposed {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let completed = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: scope("team"),
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        let turn_id = match completed {
            ToolCallOutcome::Completed { tool_call } => tool_call.request.turn_id,
            _ => panic!("completion expected"),
        };
        fs::write(dir.path().join("new.txt"), "human edit").unwrap();
        assert!(matches!(
            service.undo_turn(&scope("team"), &turn_id).await,
            Err(ExecutionError::Tool(ToolError::ConcurrentModification))
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "human edit"
        );
    }

    #[tokio::test]
    async fn undo_moves_only_the_latest_turn_out_of_conversation_time() {
        let (_dir, service, session_id) = service().await;
        let team = scope("team");
        let first = service.store.create_turn(&team, &session_id).await.unwrap();
        service
            .store
            .append_turn_message(
                &team,
                &session_id,
                &first.id,
                "user",
                json!("first question"),
            )
            .await
            .unwrap();
        service
            .store
            .append_turn_message(
                &team,
                &session_id,
                &first.id,
                "assistant",
                json!("first answer"),
            )
            .await
            .unwrap();
        service
            .store
            .update_turn(
                &team,
                &first.id,
                s_code_protocol::TurnStatus::Completed,
                None,
                None,
            )
            .await
            .unwrap();
        let original_goal = service
            .store
            .set_session_goal(
                &session_id,
                SetSessionGoal {
                    scope: team.clone(),
                    objective: "finish the feature".into(),
                    auto_continue: true,
                    token_budget: None,
                },
            )
            .await
            .unwrap();
        let second = service.store.create_turn(&team, &session_id).await.unwrap();
        service
            .store
            .begin_session_goal_checkpoint(&team, &session_id, &second.id, Some(&original_goal))
            .await
            .unwrap();
        let paused_goal = service
            .store
            .update_session_goal(
                &session_id,
                UpdateSessionGoal {
                    scope: team.clone(),
                    objective: None,
                    status: Some(SessionGoalStatus::Paused),
                    auto_continue: None,
                    expected_revision: original_goal.revision,
                    blocked_reason: None,
                },
            )
            .await
            .unwrap();
        service
            .store
            .finish_session_goal_checkpoint(&team, &session_id, &second.id, Some(&paused_goal))
            .await
            .unwrap();
        service
            .store
            .append_turn_message(
                &team,
                &session_id,
                &second.id,
                "user",
                json!("second question"),
            )
            .await
            .unwrap();
        let queued = service
            .store
            .create_turn_input(
                &session_id,
                CreateTurnInput {
                    scope: team.clone(),
                    target_turn_id: second.id.clone(),
                    mode: TurnInputMode::Queue,
                    content: json!("queued follow-up"),
                    idempotency_key: "undo-queued-follow-up".into(),
                },
            )
            .await
            .unwrap();
        service
            .store
            .append_turn_message(
                &team,
                &session_id,
                &second.id,
                "assistant",
                json!("second answer"),
            )
            .await
            .unwrap();
        service
            .store
            .update_turn(
                &team,
                &second.id,
                s_code_protocol::TurnStatus::Completed,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(matches!(
            service.undo_turn(&team, &first.id).await,
            Err(ExecutionError::Arguments(message)) if message.contains("latest conversation Turn")
        ));
        let undone = service.undo_turn(&team, &second.id).await.unwrap();
        assert_eq!(undone.conversation_omitted_messages, 2);
        assert!(undone.session_goal_rewound);
        assert_eq!(
            service
                .store
                .get_session_goal(&team, &session_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            SessionGoalStatus::Active
        );
        assert_eq!(undone.cancelled_input_ids, vec![queued.id.clone()]);
        assert_eq!(
            service
                .store
                .get_turn_input(&team, &queued.id)
                .await
                .unwrap()
                .status,
            TurnInputStatus::Cancelled
        );
        let history = service
            .store
            .list_messages(&team, &session_id)
            .await
            .unwrap();
        let marker = history
            .iter()
            .find(|message| message.role == "conversation_undo_state")
            .unwrap();
        assert_eq!(marker.content["target_turn_id"], second.id.0);
        assert!(matches!(
            service.undo_turn(&team, &second.id).await,
            Err(ExecutionError::Arguments(message)) if message.contains("already")
        ));
    }

    #[tokio::test]
    async fn denied_tools_never_execute() {
        let (_dir, service, session) = service().await;
        let outcome = service
            .submit(
                &session,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "git_force_push".into(),
                    arguments: json!({}),
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ToolCallOutcome::Denied { .. }));
    }

    #[tokio::test]
    async fn external_scm_write_cannot_bypass_approval() {
        let (_dir, service, session) = service().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let service = service.with_external(Arc::new(FakeExternal {
            calls: calls.clone(),
        }));
        let mut team_scope = scope("team");
        team_scope.task_id = Some(Id("task".into()));
        let turn = service
            .store
            .create_turn(&team_scope, &session)
            .await
            .unwrap();
        let outcome = service
            .submit_for_turn(
                &session,
                &turn.id,
                SubmitToolCall {
                    scope: team_scope.clone(),
                    tool: "scm_create_draft_pr".into(),
                    arguments: json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let approval = match outcome {
            ToolCallOutcome::AwaitingApproval { approval, .. } => approval,
            _ => panic!("approval expected"),
        };
        let outcome = service
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: team_scope,
                    approved: true,
                    approval_scope: ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ToolCallOutcome::Completed { .. }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn valid_cached_team_policy_applies_but_cannot_override_local_deny() {
        use s_code_policy::{
            CentralTeamConfigurationPayload, Rule, SignedTeamConfiguration,
            TeamRuntimeConfiguration,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team"),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "central policy".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let central = PolicyBundle {
            id: "central".into(),
            version: "9".into(),
            rules: vec![
                Rule {
                    tool: "read_file".into(),
                    decision: PolicyDecision::Deny,
                    reason: "central deny".into(),
                },
                Rule {
                    tool: "git_force_push".into(),
                    decision: PolicyDecision::Allow,
                    reason: "attempted weakening".into(),
                },
            ],
            default: PolicyDecision::Ask,
        };
        store
            .apply_verified_team_configuration(&SignedTeamConfiguration {
                key_id: "verified-elsewhere".into(),
                signature: "bound-signature".into(),
                payload: CentralTeamConfigurationPayload {
                    organization_id: Id("org".into()),
                    team_id: Id("team".into()),
                    sequence: 1,
                    issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                    expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                    configuration: TeamRuntimeConfiguration {
                        human_available_hours: 1.0,
                        agent_concurrency: 1,
                        wip_limit: 1,
                        allowed_model_ids: vec![],
                        model_routing_order: vec![],
                        model_fallback_reasons: vec![],
                        policy_sequence: 9,
                        policy_bundle: central,
                        policy_rollout_percent: 100,
                        policy_rollout_seed: "all-devices".into(),
                        policy_simulate: false,
                        knowledge_version: "v1".into(),
                        audit_content_policy: None,
                    },
                },
            })
            .await
            .unwrap();
        let execution =
            ExecutionService::new(store, PolicyBundle::default(), Arc::new(FakeRuntime));
        for tool in ["read_file", "git_force_push"] {
            let outcome = execution
                .submit(
                    &session.id,
                    SubmitToolCall {
                        scope: scope("team"),
                        tool: tool.into(),
                        arguments: json!({"path":"a.txt"}),
                    },
                )
                .await
                .unwrap();
            assert!(matches!(outcome, ToolCallOutcome::Denied { .. }));
        }
    }

    #[tokio::test]
    async fn central_policy_simulation_is_persisted_without_changing_effective_decision() {
        use s_code_policy::{
            CentralTeamConfigurationPayload, Rule, SignedTeamConfiguration,
            TeamRuntimeConfiguration,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team"),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "simulation".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let simulated = PolicyBundle {
            id: "simulated".into(),
            version: "2".into(),
            rules: vec![Rule {
                tool: "read_file".into(),
                decision: PolicyDecision::Deny,
                reason: "would deny".into(),
            }],
            default: PolicyDecision::Ask,
        };
        store
            .apply_verified_team_configuration(&SignedTeamConfiguration {
                key_id: "verified".into(),
                signature: "signature".into(),
                payload: CentralTeamConfigurationPayload {
                    organization_id: Id("org".into()),
                    team_id: Id("team".into()),
                    sequence: 2,
                    issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                    expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                    configuration: TeamRuntimeConfiguration {
                        human_available_hours: 1.0,
                        agent_concurrency: 1,
                        wip_limit: 1,
                        allowed_model_ids: vec![],
                        model_routing_order: vec![],
                        model_fallback_reasons: vec![],
                        policy_sequence: 2,
                        policy_bundle: simulated,
                        policy_rollout_percent: 100,
                        policy_rollout_seed: "simulation".into(),
                        policy_simulate: true,
                        knowledge_version: "v1".into(),
                        audit_content_policy: None,
                    },
                },
            })
            .await
            .unwrap();
        let execution = ExecutionService::new(
            store.clone(),
            PolicyBundle::default(),
            Arc::new(FakeRuntime),
        )
        .with_device_id(Id("device-a".into()));
        let outcome = execution
            .submit(
                &session.id,
                SubmitToolCall {
                    scope: scope("team"),
                    tool: "read_file".into(),
                    arguments: json!({"path":"a.txt"}),
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Completed { tool_call } = outcome else {
            panic!("local allow must remain effective")
        };
        let persisted = store.get_tool_call(&tool_call.request.id).await.unwrap();
        assert_eq!(tool_call.policy.decision, PolicyDecision::Allow);
        assert_eq!(
            tool_call.simulated_policy.as_ref().unwrap().decision,
            PolicyDecision::Deny
        );
        assert!(!tool_call.central_policy_applied);
        assert_eq!(tool_call.team_configuration_sequence, Some(2));
        assert_eq!(persisted, tool_call);
    }

    #[tokio::test]
    async fn scoped_exception_preapproves_ask_but_never_overrides_deny() {
        use s_code_policy::{CentralPolicyExceptionPayload, SignedPolicyExceptionGrant};
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team"),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "exception".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let grant = |id: &str, tool: &str| SignedPolicyExceptionGrant {
            key_id: "verified".into(),
            signature: "verified-before-storage".into(),
            payload: CentralPolicyExceptionPayload {
                organization_id: session.scope.organization_id.clone(),
                team_id: session.scope.team_id.clone(),
                exception_id: Id(id.into()),
                sequence: 1,
                issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                active: true,
                tool: tool.into(),
                actor_id: Some(session.scope.actor_id.clone()),
                workspace_uri: Some(session.workspace_uri.clone()),
                reason: "approved maintenance".into(),
                requested_by: session.scope.actor_id.clone(),
                approved_by: Id("approver".into()),
            },
        };
        store
            .apply_verified_policy_exception(&grant("exception-ask", "future_tool"))
            .await
            .unwrap();
        store
            .apply_verified_policy_exception(&grant("exception-deny", "git_force_push"))
            .await
            .unwrap();
        let execution =
            ExecutionService::new(store, PolicyBundle::default(), Arc::new(FakeRuntime));
        let outcome = execution
            .submit(
                &session.id,
                SubmitToolCall {
                    scope: session.scope.clone(),
                    tool: "future_tool".into(),
                    arguments: json!({}),
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Failed { tool_call } = outcome else {
            panic!("preapproved unknown tool reaches dispatch and fails as unsupported")
        };
        assert_eq!(tool_call.policy.decision, PolicyDecision::Allow);
        assert_eq!(tool_call.policy.policy_id, "exception:exception-ask");

        let denied = execution
            .submit(
                &session.id,
                SubmitToolCall {
                    scope: session.scope,
                    tool: "git_force_push".into(),
                    arguments: json!({}),
                },
            )
            .await
            .unwrap();
        let ToolCallOutcome::Denied { tool_call } = denied else {
            panic!("mandatory deny must win")
        };
        assert_eq!(tool_call.policy.decision, PolicyDecision::Deny);
        assert!(!tool_call.policy.policy_id.starts_with("exception:"));
    }
}
