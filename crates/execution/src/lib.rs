use opencoding_git_automation::{CommitRequest, GitService, PushRequest};
use opencoding_platform_runtime::PlatformRuntime;
use opencoding_policy::{PolicyBundle, applies_to_rollout};
use opencoding_protocol::{
    ApprovalStatus, Id, PolicyDecision, ResolveApproval, SubmitToolCall, ToolCall, ToolCallOutcome,
    ToolCallStatus, ToolRequest,
};
use opencoding_storage::{StorageError, Store, ToolPolicyMetadata};
use opencoding_tool_runtime::{FileReplacement, ToolError, ToolRuntime, content_sha256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use thiserror::Error;

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
    policy: opencoding_protocol::PolicyResult,
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
                opencoding_protocol::TurnStatus::PreparingContext,
                None,
                None,
            )
            .await?;
        self.store
            .update_turn(
                &input.scope,
                &turn.id,
                opencoding_protocol::TurnStatus::CallingModel,
                None,
                None,
            )
            .await?;
        let marker = serde_json::json!({"execution_mode":"manual_tool"});
        self.store
            .update_turn(
                &input.scope,
                &turn.id,
                opencoding_protocol::TurnStatus::RunningTool,
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

    async fn evaluate_policy(
        &self,
        request: &ToolRequest,
        workspace_uri: &str,
    ) -> Result<(opencoding_protocol::PolicyResult, ToolPolicyMetadata), ExecutionError> {
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
                Some(_) => opencoding_protocol::PolicyResult {
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
            policy = opencoding_protocol::PolicyResult {
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
                        opencoding_protocol::TurnStatus::Failed,
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
                    opencoding_protocol::TurnStatus::RunningTool,
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
        scope: &opencoding_protocol::Scope,
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
                        opencoding_protocol::TurnStatus::AwaitingApproval,
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
                        opencoding_protocol::TurnStatus::CallingModel,
                        None,
                        None,
                    )
                    .await?;
                self.store
                    .update_turn(
                        scope,
                        turn_id,
                        opencoding_protocol::TurnStatus::Completed,
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
                        opencoding_protocol::TurnStatus::Failed,
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
        scope: &opencoding_protocol::Scope,
        turn_id: &Id,
    ) -> Result<UndoTurnResult, ExecutionError> {
        let turn = self.store.get_turn(scope, turn_id).await?;
        if !matches!(
            turn.status,
            opencoding_protocol::TurnStatus::Completed
                | opencoding_protocol::TurnStatus::Failed
                | opencoding_protocol::TurnStatus::Cancelled
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
                let value = opencoding_audit::redact(value);
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
                let message = error.to_string();
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
                Ok(serde_json::to_value(runtime.read_file(
                    &args.path,
                    args.start_line.unwrap_or(1),
                    args.end_line.unwrap_or(200),
                    args.max_bytes.unwrap_or(128 * 1024),
                )?)
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "search_text" => {
                let args: SearchArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    runtime
                        .search_text(
                            &args.query,
                            args.glob.as_deref(),
                            args.max_bytes.unwrap_or(256 * 1024),
                        )
                        .await?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
            "apply_patch" => {
                let replacement: FileReplacement = args(&call.request.arguments)?;
                let snapshot = runtime.snapshot_file(&replacement.path)?;
                match (&snapshot.sha256, &replacement.expected_sha256) {
                    (Some(current), Some(expected)) if current == expected => {}
                    (Some(_), None) => return Err(ToolError::MissingExpectedHash.into()),
                    (None, None) => {}
                    _ => return Err(ToolError::ConcurrentModification.into()),
                }
                let after_sha256 = content_sha256(replacement.content.as_bytes());
                let plan = self
                    .store
                    .plan_turn_file_change(
                        &call.request.scope,
                        &call.request.turn_id,
                        &replacement.path,
                        snapshot.content.as_deref(),
                        snapshot.sha256.as_deref(),
                        &after_sha256,
                    )
                    .await?;
                match runtime.apply_replacement(replacement) {
                    Ok(result) => {
                        self.store.complete_turn_file_change(&plan).await?;
                        Ok(serde_json::to_value(result)
                            .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
                    }
                    Err(error) => {
                        self.store.abort_turn_file_change(&plan).await?;
                        Err(error.into())
                    }
                }
            }
            "run_command" => {
                let args: RunArgs = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    runtime
                        .run_with_profile(
                            &args.program,
                            args.args,
                            Duration::from_secs(args.timeout_seconds.unwrap_or(60).min(600)),
                            args.network_enabled.unwrap_or(false),
                            args.max_bytes.unwrap_or(1024 * 1024),
                            args.sandbox_profile.workspace_writable(),
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
                            args.max_bytes.unwrap_or(1024 * 1024),
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
            "git_push" => {
                let request: PushRequest = args(&call.request.arguments)?;
                Ok(serde_json::to_value(
                    GitService::open(&session.workspace_uri)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?
                        .push(request)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                )
                .map_err(|error| ExecutionError::Arguments(error.to_string()))?)
            }
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
            let _: FileReplacement = args(value)?;
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
            let _: PushRequest = args(value)?;
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
}

impl RunSandboxProfile {
    fn workspace_writable(self) -> bool {
        matches!(self, Self::WorkspaceWrite)
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
    use opencoding_platform_runtime::{ProcessOutput, ProcessSpec, RuntimeError};
    use opencoding_protocol::{
        ApprovalScope, CreateSession, CreateTurnInput, Scope, SessionGoalStatus, SetSessionGoal,
        TurnInputMode, TurnInputStatus, UpdateSessionGoal,
    };
    use serde_json::json;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct FakeRuntime;

    struct FakeExternal {
        calls: Arc<AtomicUsize>,
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

        assert!(
            serde_json::from_value::<RunArgs>(json!({
                "program": "cargo",
                "sandbox_profile": "host-unrestricted"
            }))
            .is_err()
        );
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
        fn capabilities(&self) -> Vec<opencoding_protocol::Capability> {
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
                opencoding_protocol::TurnStatus::Completed,
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
                opencoding_protocol::TurnStatus::Completed,
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
        use opencoding_policy::{
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
        use opencoding_policy::{
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
        use opencoding_policy::{CentralPolicyExceptionPayload, SignedPolicyExceptionGrant};
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
