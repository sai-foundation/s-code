use async_trait::async_trait;
use futures_util::{StreamExt, future::join_all};
use s_code_context_engine::{
    ConversationMessage, estimate_conversation_tokens, pack_conversation_history,
};
use s_code_model_gateway::{
    GatewayError, ModelEvent, ModelMessage, ModelProvider, ModelRequest, ModelRoutingPolicy,
    ToolDefinition,
};
use s_code_protocol::TurnStatus;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub mod step;
pub mod tool;

use step::{
    AdmissionTarget, QueuePosition, StepAdmission, StepRequest, StepRequestOptions,
    StepRequestQueue,
};
use tool::{ResourceClaim, ToolSchedule};

#[derive(Clone, Debug)]
pub struct TurnLimits {
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    pub max_elapsed_seconds: u64,
    pub model_stream_idle_seconds: u64,
    pub max_cost_micros: u64,
    pub max_total_tokens: u64,
}

impl Default for TurnLimits {
    fn default() -> Self {
        Self {
            max_model_calls: 48,
            max_tool_calls: 64,
            max_elapsed_seconds: 1800,
            model_stream_idle_seconds: 60,
            max_cost_micros: 5_000_000,
            max_total_tokens: 1_000_000,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TurnError {
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: TurnStatus, to: TurnStatus },
    #[error("turn limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

#[derive(Clone, Debug)]
pub struct TurnMachine {
    status: TurnStatus,
    limits: TurnLimits,
    model_calls: u32,
    tool_calls: u32,
    cost_micros: u64,
    total_tokens: u64,
    started_at: Instant,
}

impl TurnMachine {
    pub fn new(limits: TurnLimits) -> Self {
        Self {
            status: TurnStatus::Idle,
            limits,
            model_calls: 0,
            tool_calls: 0,
            cost_micros: 0,
            total_tokens: 0,
            started_at: Instant::now(),
        }
    }
    pub fn status(&self) -> &TurnStatus {
        &self.status
    }
    pub fn record_model_call(&mut self, cost_micros: u64) -> Result<(), TurnError> {
        self.model_calls += 1;
        self.cost_micros = self.cost_micros.saturating_add(cost_micros);
        if self.model_calls > self.limits.max_model_calls {
            return Err(TurnError::LimitExceeded("model_calls"));
        }
        if self.cost_micros > self.limits.max_cost_micros {
            return Err(TurnError::LimitExceeded("cost"));
        }
        Ok(())
    }
    pub fn record_tool_call(&mut self) -> Result<(), TurnError> {
        self.tool_calls += 1;
        if self.tool_calls > self.limits.max_tool_calls {
            return Err(TurnError::LimitExceeded("tool_calls"));
        }
        Ok(())
    }
    pub fn record_usage(&mut self, input_tokens: u64, output_tokens: u64) -> Result<(), TurnError> {
        self.total_tokens = self
            .total_tokens
            .saturating_add(input_tokens)
            .saturating_add(output_tokens);
        if self.total_tokens > self.limits.max_total_tokens {
            return Err(TurnError::LimitExceeded("tokens"));
        }
        Ok(())
    }
    pub fn check_elapsed(&self) -> Result<(), TurnError> {
        if self.started_at.elapsed().as_secs() > self.limits.max_elapsed_seconds {
            return Err(TurnError::LimitExceeded("elapsed_time"));
        }
        Ok(())
    }
    pub fn transition(&mut self, to: TurnStatus) -> Result<(), TurnError> {
        if !valid(&self.status, &to) {
            return Err(TurnError::InvalidTransition {
                from: self.status.clone(),
                to,
            });
        }
        self.status = to;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRunRequest {
    pub model: String,
    pub temperature: f32,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ModelRoutingPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentRunStatus {
    Completed,
    AwaitingInput { detail: Value },
    AwaitingApproval { detail: Value },
    Failed { reason: String },
    Cancelled,
}

pub const MODEL_STREAM_IDLE_TIMEOUT_REASON: &str = "model stream idle timeout";
pub const TURN_ELAPSED_TIMEOUT_REASON: &str = "turn elapsed-time limit exceeded";
pub const EMPTY_MODEL_RESPONSE_REASON: &str = "model returned repeated empty responses";
pub const MODEL_CALL_LIMIT_REASON: &str = "model-call limit reached";
pub const TOOL_CALL_LIMIT_REASON: &str = "tool-call limit reached";
pub const INCOMPLETE_MODEL_RESPONSE_REASON: &str =
    "model repeatedly returned truncated output or provider tool markup as text";
const MAX_EMPTY_MODEL_RETRIES: u32 = 2;
const MAX_INCOMPLETE_MODEL_RETRIES: u32 = 2;
const MAX_MODEL_STREAM_RETRIES: u32 = 2;
const MAX_ADAPTIVE_OUTPUT_TOKENS: u32 = 32_768;
const RECENT_DETAILED_TOOL_RESULTS: usize = 6;
const BUDGET_CONVERGENCE_REMINDER: &str = "The Turn is approaching its execution budget. If the requested work is already verified, stop using tools and return the verified result. Otherwise perform only the highest-value remaining action, then verify and conclude. Do not weaken tests or claim unobserved success.";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentRunResult {
    pub status: AgentRunStatus,
    pub assistant_text: String,
    pub messages: Vec<ModelMessage>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u32,
    pub tool_calls: u32,
    #[serde(default)]
    pub ops: Vec<AgentOp>,
}

pub const AGENT_OP_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOp {
    pub schema_version: u32,
    pub sequence: u64,
    pub operation: AgentOperation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOperation {
    StatusChanged {
        status: TurnStatus,
    },
    ModelCallStarted,
    TextAppended {
        text: String,
    },
    UsageAdded {
        input_tokens: u64,
        output_tokens: u64,
    },
    ToolCallStarted {
        call_id: String,
        tool: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentProjection {
    pub last_sequence: u64,
    pub status: Option<TurnStatus>,
    pub assistant_text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model_calls: u32,
    pub tool_calls: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AgentOpError {
    #[error("unsupported Agent operation schema version {0}")]
    UnsupportedVersion(u32),
    #[error("Agent operation sequence gap: expected {expected}, received {received}")]
    SequenceGap { expected: u64, received: u64 },
}

impl AgentProjection {
    pub fn apply(&mut self, operation: &AgentOp) -> Result<(), AgentOpError> {
        if operation.schema_version != AGENT_OP_SCHEMA_VERSION {
            return Err(AgentOpError::UnsupportedVersion(operation.schema_version));
        }
        let expected = self.last_sequence.saturating_add(1);
        if operation.sequence != expected {
            return Err(AgentOpError::SequenceGap {
                expected,
                received: operation.sequence,
            });
        }
        match &operation.operation {
            AgentOperation::StatusChanged { status } => self.status = Some(status.clone()),
            AgentOperation::ModelCallStarted => {
                self.model_calls = self.model_calls.saturating_add(1)
            }
            AgentOperation::TextAppended { text } => self.assistant_text.push_str(text),
            AgentOperation::UsageAdded {
                input_tokens,
                output_tokens,
            } => {
                self.input_tokens = self.input_tokens.saturating_add(*input_tokens);
                self.output_tokens = self.output_tokens.saturating_add(*output_tokens);
            }
            AgentOperation::ToolCallStarted { .. } => {
                self.tool_calls = self.tool_calls.saturating_add(1)
            }
        }
        self.last_sequence = operation.sequence;
        Ok(())
    }
}

pub fn replay_agent_ops(operations: &[AgentOp]) -> Result<AgentProjection, AgentOpError> {
    let mut projection = AgentProjection::default();
    for operation in operations {
        projection.apply(operation)?;
    }
    Ok(projection)
}

#[derive(Default)]
struct AgentJournal {
    projection: AgentProjection,
    operations: Vec<AgentOp>,
}

impl AgentJournal {
    fn append(&mut self, operation: AgentOperation) {
        let operation = AgentOp {
            schema_version: AGENT_OP_SCHEMA_VERSION,
            sequence: self.projection.last_sequence.saturating_add(1),
            operation,
        };
        self.projection
            .apply(&operation)
            .expect("live Agent operations are contiguous and use the current schema");
        self.operations.push(operation);
    }
}

pub const AGENT_CHECKPOINT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCheckpoint {
    pub schema_version: u32,
    pub result: AgentRunResult,
}

#[derive(Debug, Error)]
pub enum AgentCheckpointError {
    #[error("unsupported Agent checkpoint schema version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid Agent checkpoint: {0}")]
    Invalid(String),
}

impl AgentCheckpoint {
    pub fn new(result: AgentRunResult) -> Self {
        Self {
            schema_version: AGENT_CHECKPOINT_SCHEMA_VERSION,
            result,
        }
    }

    pub fn decode(value: Value) -> Result<Self, AgentCheckpointError> {
        if value.get("schema_version").is_none() {
            let result = serde_json::from_value(value)
                .map_err(|error| AgentCheckpointError::Invalid(error.to_string()))?;
            return Ok(Self::new(result));
        }
        let checkpoint: Self = serde_json::from_value(value)
            .map_err(|error| AgentCheckpointError::Invalid(error.to_string()))?;
        if checkpoint.schema_version != AGENT_CHECKPOINT_SCHEMA_VERSION {
            return Err(AgentCheckpointError::UnsupportedVersion(
                checkpoint.schema_version,
            ));
        }
        if !checkpoint.result.ops.is_empty() {
            let projection = replay_agent_ops(&checkpoint.result.ops)
                .map_err(|error| AgentCheckpointError::Invalid(error.to_string()))?;
            if projection.assistant_text != checkpoint.result.assistant_text
                || projection.input_tokens != checkpoint.result.input_tokens
                || projection.output_tokens != checkpoint.result.output_tokens
                || projection.model_calls != checkpoint.result.model_calls
                || projection.tool_calls != checkpoint.result.tool_calls
            {
                return Err(AgentCheckpointError::Invalid(
                    "operation replay does not match the stored Agent result".into(),
                ));
            }
        }
        Ok(checkpoint)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AgentToolResult {
    Completed {
        value: Value,
    },
    /// Host-authorized change of execution context, never parsed from model output.
    ContextChanged {
        retired_system_content: Value,
        value: Value,
        tools: Vec<ToolDefinition>,
        system_messages: Vec<ModelMessage>,
    },
    DiscoveredTools {
        value: Value,
        tools: Vec<ToolDefinition>,
    },
    AwaitingInput {
        detail: Value,
    },
    AwaitingApproval {
        detail: Value,
    },
    Failed {
        error: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Status {
        status: TurnStatus,
    },
    TextDelta {
        text: String,
    },
    ReasoningSummaryDelta {
        text: String,
    },
    ToolFinished {
        call_id: String,
        tool: String,
        success: bool,
    },
    ToolProposed {
        call_id: String,
        tool: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    ModelRouteSelected {
        model_id: String,
        fallback_from: Option<String>,
        reason: Option<s_code_model_gateway::FallbackReason>,
    },
    ModelRouteFallback {
        from_model_id: String,
        to_model_id: String,
        reason: s_code_model_gateway::FallbackReason,
    },
    ContextCompacted {
        omitted_messages: u32,
        truncated_messages: u32,
        estimated_tokens: u64,
    },
}

pub trait AgentObserver: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

struct NoopObserver;

impl AgentObserver for NoopObserver {
    fn emit(&self, _: AgentEvent) {}
}

#[async_trait]
pub trait AgentToolExecutor: Send + Sync {
    /// Execution-backed tools persist their own lifecycle; host-only tools can
    /// ask the runner to publish a result after their proposed event.
    fn owns_tool_lifecycle(&self, _tool: &str) -> bool {
        true
    }

    /// Interactive tools remain exclusive so a pause cannot strand another
    /// call in an in-memory scheduling wave. Executors may opt out only when
    /// preparation proves the call cannot request approval or user input.
    fn may_pause(&self, _tool: &str, _arguments: &Value) -> bool {
        true
    }

    fn resource_claims(&self, _tool: &str, _arguments: &Value) -> Vec<ResourceClaim> {
        vec![ResourceClaim::global_exclusive()]
    }

    async fn prepare(
        &self,
        _call_id: &str,
        tool: &str,
        arguments: Value,
        _cancellation: &CancellationToken,
    ) -> PreparedAgentToolCall {
        PreparedAgentToolCall::ready(
            arguments.clone(),
            self.resource_claims(tool, &arguments),
            self.may_pause(tool, &arguments),
        )
    }

    async fn execute_prepared(
        &self,
        call_id: &str,
        tool: &str,
        prepared: PreparedAgentToolCall,
        cancellation: &CancellationToken,
    ) -> AgentToolResult {
        if let Some(result) = prepared.resolved {
            return result;
        }
        self.execute(call_id, tool, prepared.arguments, cancellation)
            .await
    }

    async fn execute(
        &self,
        call_id: &str,
        tool: &str,
        arguments: Value,
        cancellation: &CancellationToken,
    ) -> AgentToolResult;
}

#[derive(Clone, Debug)]
pub struct PreparedAgentToolCall {
    arguments: Value,
    claims: Vec<ResourceClaim>,
    may_pause: bool,
    resolved: Option<AgentToolResult>,
}

impl PreparedAgentToolCall {
    pub fn ready(arguments: Value, claims: Vec<ResourceClaim>, may_pause: bool) -> Self {
        Self {
            arguments,
            claims,
            may_pause,
            resolved: None,
        }
    }

    pub fn resolved(result: AgentToolResult) -> Self {
        Self {
            arguments: Value::Null,
            claims: vec![ResourceClaim::global_exclusive()],
            may_pause: false,
            resolved: Some(result),
        }
    }

    pub fn into_arguments(self) -> Result<Value, AgentToolResult> {
        match self.resolved {
            Some(result) => Err(result),
            None => Ok(self.arguments),
        }
    }

    pub fn arguments(&self) -> Option<&Value> {
        self.resolved.is_none().then_some(&self.arguments)
    }

    pub fn claims(&self) -> &[ResourceClaim] {
        &self.claims
    }

    pub fn may_pause(&self) -> bool {
        self.may_pause
    }
}

pub struct AgentRunner {
    provider: Arc<dyn ModelProvider>,
    executor: Arc<dyn AgentToolExecutor>,
    limits: TurnLimits,
    max_provider_retries: u32,
    repeated_failure_limit: u32,
    observer: Arc<dyn AgentObserver>,
}

impl AgentRunner {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        executor: Arc<dyn AgentToolExecutor>,
        limits: TurnLimits,
    ) -> Self {
        Self {
            provider,
            executor,
            limits,
            max_provider_retries: 2,
            repeated_failure_limit: 3,
            observer: Arc::new(NoopObserver),
        }
    }

    pub fn with_observer(mut self, observer: Arc<dyn AgentObserver>) -> Self {
        self.observer = observer;
        self
    }

    pub async fn run(
        &self,
        request: AgentRunRequest,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, AgentError> {
        self.run_inner(request, cancellation, None).await
    }

    pub async fn run_with_step_ingress(
        &self,
        request: AgentRunRequest,
        cancellation: CancellationToken,
        ingress: mpsc::Receiver<StepRequest>,
    ) -> Result<AgentRunResult, AgentError> {
        self.run_inner(request, cancellation, Some(ingress)).await
    }

    async fn run_inner(
        &self,
        mut request: AgentRunRequest,
        cancellation: CancellationToken,
        mut ingress: Option<mpsc::Receiver<StepRequest>>,
    ) -> Result<AgentRunResult, AgentError> {
        if !request.temperature.is_finite() || !(0.0..=2.0).contains(&request.temperature) {
            return Err(AgentError::InvalidRequest(
                "temperature must be finite and between 0 and 2".into(),
            ));
        }
        let mut machine = TurnMachine::new(self.limits.clone());
        let mut journal = AgentJournal::default();
        machine.transition(TurnStatus::PreparingContext)?;
        self.emit_status(&machine, &mut journal);
        machine.transition(TurnStatus::CallingModel)?;
        self.emit_status(&machine, &mut journal);
        let mut assistant_text = String::new();
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut model_calls = 0_u32;
        let mut tool_calls = 0_u32;
        let mut consecutive_empty_model_responses = 0_u32;
        let mut consecutive_incomplete_model_responses = 0_u32;
        let mut consecutive_model_stream_retries = 0_u32;
        let mut budget_convergence_reminder_sent = false;
        let mut failures: BTreeMap<String, u32> = BTreeMap::new();
        let mut step_queue = initial_step_queue(&mut request.messages);
        let turn_deadline =
            tokio::time::Instant::now() + Duration::from_secs(self.limits.max_elapsed_seconds);

        loop {
            drain_external_steps(&mut ingress, &mut step_queue);
            if cancellation.is_cancelled() {
                materialize_cancelled_steps(&mut step_queue, &mut request.messages);
                machine.transition(TurnStatus::Cancelled)?;
                self.emit_status(&machine, &mut journal);
                return Ok(result(
                    AgentRunStatus::Cancelled,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                    journal,
                ));
            }
            let Some(batch) = step_queue.take_next_batch() else {
                machine.transition(TurnStatus::Completed)?;
                self.emit_status(&machine, &mut journal);
                return Ok(result(
                    AgentRunStatus::Completed,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                    journal,
                ));
            };
            request.messages.extend(batch.materialize());
            if model_calls >= self.limits.max_model_calls {
                materialize_pending_steps(&mut step_queue, &mut request.messages);
                return self.fail_run(
                    &mut machine,
                    &mut journal,
                    MODEL_CALL_LIMIT_REASON,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                );
            }
            let convergence_threshold = self.limits.max_model_calls.saturating_mul(4).div_ceil(5);
            if !budget_convergence_reminder_sent && model_calls >= convergence_threshold {
                request.messages.push(ModelMessage {
                    role: "system".into(),
                    content: Value::String(BUDGET_CONVERGENCE_REMINDER.into()),
                });
                budget_convergence_reminder_sent = true;
            }
            compact_superseded_tool_history(&mut request.messages);
            machine.record_model_call(0)?;
            model_calls += 1;
            journal.append(AgentOperation::ModelCallStarted);
            let mut model_request = ModelRequest {
                model: request.model.clone(),
                temperature: request.temperature,
                messages: request.messages.clone(),
                tools: request.tools.clone(),
                max_output_tokens: request.max_output_tokens,
                routing: request.routing.clone(),
            };
            let idle_timeout = Duration::from_secs(self.limits.model_stream_idle_seconds);
            let stream_result = tokio::select! {
                () = cancellation.cancelled() => Err(AgentError::Cancelled),
                () = tokio::time::sleep_until(turn_deadline) => {
                    materialize_pending_steps(&mut step_queue, &mut request.messages);
                    return self.fail_run(
                        &mut machine,
                        &mut journal,
                        TURN_ELAPSED_TIMEOUT_REASON,
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                    );
                }
                () = tokio::time::sleep(idle_timeout) => {
                    materialize_pending_steps(&mut step_queue, &mut request.messages);
                    return self.fail_run(
                        &mut machine,
                        &mut journal,
                        MODEL_STREAM_IDLE_TIMEOUT_REASON,
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                    );
                }
                stream = self.stream_with_retry(&mut model_request, &cancellation) => stream,
            };
            let mut stream = match stream_result {
                Ok(stream) => stream,
                Err(AgentError::Cancelled) => {
                    machine.transition(TurnStatus::Cancelled)?;
                    self.emit_status(&machine, &mut journal);
                    return Ok(result(
                        AgentRunStatus::Cancelled,
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                        journal,
                    ));
                }
                Err(error) => return Err(error),
            };
            request.messages = model_request.messages;
            let mut text = String::new();
            let mut calls: BTreeMap<u32, ToolCallBuilder> = BTreeMap::new();
            let mut call_output_tokens = 0_u64;
            let mut finish_reason = None;
            let idle_sleep = tokio::time::sleep(idle_timeout);
            tokio::pin!(idle_sleep);
            let mut stream_failure = None;
            let mut stream_error = None;
            loop {
                let event = tokio::select! {
                    () = cancellation.cancelled() => {
                        preserve_partial_model_text(
                            &mut text,
                            &mut assistant_text,
                            &mut request.messages,
                            &mut journal,
                        );
                        materialize_cancelled_steps(&mut step_queue, &mut request.messages);
                        machine.transition(TurnStatus::Cancelled)?;
                        self.emit_status(&machine, &mut journal);
                        return Ok(result(
                            AgentRunStatus::Cancelled,
                            assistant_text,
                            request.messages,
                            input_tokens,
                            output_tokens,
                            model_calls,
                            tool_calls,
                            journal,
                        ));
                    }
                    () = tokio::time::sleep_until(turn_deadline) => {
                        stream_failure = Some(TURN_ELAPSED_TIMEOUT_REASON);
                        break;
                    }
                    () = &mut idle_sleep => {
                        stream_failure = Some(MODEL_STREAM_IDLE_TIMEOUT_REASON);
                        break;
                    }
                    request = receive_external_step(&mut ingress) => {
                        step_queue
                            .admit(request, true, QueuePosition::Tail)
                            .map_err(|error| AgentError::InvalidRequest(error.to_string()))?;
                        continue;
                    }
                    event = stream.next() => event,
                };
                let Some(event) = event else {
                    break;
                };
                if cancellation.is_cancelled() {
                    preserve_partial_model_text(
                        &mut text,
                        &mut assistant_text,
                        &mut request.messages,
                        &mut journal,
                    );
                    materialize_cancelled_steps(&mut step_queue, &mut request.messages);
                    machine.transition(TurnStatus::Cancelled)?;
                    self.emit_status(&machine, &mut journal);
                    return Ok(result(
                        AgentRunStatus::Cancelled,
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                        journal,
                    ));
                }
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        stream_error = Some(error);
                        break;
                    }
                };
                idle_sleep
                    .as_mut()
                    .reset(tokio::time::Instant::now() + idle_timeout);
                match event {
                    ModelEvent::TextDelta { text: delta } => {
                        if delta.is_empty() {
                            continue;
                        }
                        self.observer.emit(AgentEvent::TextDelta {
                            text: delta.clone(),
                        });
                        text.push_str(&delta);
                    }
                    ModelEvent::ReasoningSummaryDelta { text } => {
                        self.observer
                            .emit(AgentEvent::ReasoningSummaryDelta { text });
                    }
                    ModelEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                        provider_metadata,
                    } => {
                        let builder = calls.entry(index).or_default();
                        if let Some(id) = id {
                            builder.id = Some(id);
                        }
                        if let Some(name) = name {
                            builder.name = Some(name);
                        }
                        if provider_metadata.is_some() {
                            builder.provider_metadata = provider_metadata;
                        }
                        builder.arguments.push_str(&arguments_delta);
                    }
                    ModelEvent::Usage {
                        input_tokens: input,
                        output_tokens: output,
                    } => {
                        machine.record_usage(input, output)?;
                        self.observer.emit(AgentEvent::Usage {
                            input_tokens: input,
                            output_tokens: output,
                        });
                        input_tokens = input_tokens.saturating_add(input);
                        output_tokens = output_tokens.saturating_add(output);
                        call_output_tokens = call_output_tokens.max(output);
                        journal.append(AgentOperation::UsageAdded {
                            input_tokens: input,
                            output_tokens: output,
                        });
                    }
                    ModelEvent::Completed {
                        finish_reason: reason,
                    } => {
                        finish_reason = reason;
                        break;
                    }
                    ModelEvent::RouteSelected {
                        model_id,
                        fallback_from,
                        reason,
                    } => self.observer.emit(AgentEvent::ModelRouteSelected {
                        model_id,
                        fallback_from,
                        reason,
                    }),
                    ModelEvent::RouteFallback {
                        from_model_id,
                        to_model_id,
                        reason,
                    } => self.observer.emit(AgentEvent::ModelRouteFallback {
                        from_model_id,
                        to_model_id,
                        reason,
                    }),
                }
            }
            if let Some(error) = stream_error {
                if text.is_empty()
                    && calls.is_empty()
                    && consecutive_model_stream_retries < MAX_MODEL_STREAM_RETRIES
                {
                    consecutive_model_stream_retries += 1;
                    enqueue_model_stream_retry(&mut step_queue);
                    continue;
                }
                preserve_partial_model_text(
                    &mut text,
                    &mut assistant_text,
                    &mut request.messages,
                    &mut journal,
                );
                return Err(AgentError::Gateway(error));
            }
            if let Some(reason) = stream_failure {
                if reason == MODEL_STREAM_IDLE_TIMEOUT_REASON
                    && text.is_empty()
                    && calls.is_empty()
                    && consecutive_model_stream_retries < MAX_MODEL_STREAM_RETRIES
                {
                    consecutive_model_stream_retries += 1;
                    enqueue_model_stream_retry(&mut step_queue);
                    continue;
                }
                preserve_partial_model_text(
                    &mut text,
                    &mut assistant_text,
                    &mut request.messages,
                    &mut journal,
                );
                materialize_pending_steps(&mut step_queue, &mut request.messages);
                return self.fail_run(
                    &mut machine,
                    &mut journal,
                    reason,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                );
            }
            consecutive_model_stream_retries = 0;
            if calls.is_empty() && text.trim().is_empty() {
                if consecutive_empty_model_responses < MAX_EMPTY_MODEL_RETRIES {
                    consecutive_empty_model_responses += 1;
                    let exhausted_budget = finish_reason.as_deref().is_some_and(|reason| {
                        matches!(reason, "length" | "max_tokens" | "max_output_tokens")
                    }) || call_output_tokens
                        >= u64::from(request.max_output_tokens);
                    if exhausted_budget && request.max_output_tokens < MAX_ADAPTIVE_OUTPUT_TOKENS {
                        request.max_output_tokens = request
                            .max_output_tokens
                            .saturating_mul(2)
                            .min(MAX_ADAPTIVE_OUTPUT_TOKENS);
                    }
                    enqueue_empty_model_retry(&mut step_queue);
                    continue;
                }
                materialize_pending_steps(&mut step_queue, &mut request.messages);
                return self.fail_run(
                    &mut machine,
                    &mut journal,
                    EMPTY_MODEL_RESPONSE_REASON,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                );
            }
            consecutive_empty_model_responses = 0;
            let exhausted_output_budget = finish_reason.as_deref().is_some_and(|reason| {
                matches!(reason, "length" | "max_tokens" | "max_output_tokens")
            }) || call_output_tokens
                >= u64::from(request.max_output_tokens);
            if exhausted_output_budget {
                if consecutive_incomplete_model_responses < MAX_INCOMPLETE_MODEL_RETRIES {
                    consecutive_incomplete_model_responses += 1;
                    if !text.is_empty() {
                        journal.append(AgentOperation::TextAppended { text: text.clone() });
                        assistant_text.push_str(&text);
                        request.messages.push(ModelMessage {
                            role: "assistant".into(),
                            content: Value::String(text),
                        });
                    }
                    if exhausted_output_budget
                        && request.max_output_tokens < MAX_ADAPTIVE_OUTPUT_TOKENS
                    {
                        request.max_output_tokens = request
                            .max_output_tokens
                            .saturating_mul(2)
                            .min(MAX_ADAPTIVE_OUTPUT_TOKENS);
                    }
                    enqueue_incomplete_model_retry(&mut step_queue);
                    continue;
                }
                preserve_partial_model_text(
                    &mut text,
                    &mut assistant_text,
                    &mut request.messages,
                    &mut journal,
                );
                materialize_pending_steps(&mut step_queue, &mut request.messages);
                return self.fail_run(
                    &mut machine,
                    &mut journal,
                    INCOMPLETE_MODEL_RESPONSE_REASON,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                );
            }
            consecutive_incomplete_model_responses = 0;
            if !text.is_empty() {
                journal.append(AgentOperation::TextAppended { text: text.clone() });
            }
            assistant_text.push_str(&text);
            if calls.is_empty() {
                request.messages.push(ModelMessage {
                    role: "assistant".into(),
                    content: Value::String(text),
                });
                drain_external_steps(&mut ingress, &mut step_queue);
                if step_queue.has_pending_requests() {
                    continue;
                }
                close_and_drain_external_steps(&mut ingress, &mut step_queue).await;
                if step_queue.has_pending_requests() {
                    continue;
                }
                machine.transition(TurnStatus::Completed)?;
                self.emit_status(&machine, &mut journal);
                return Ok(result(
                    AgentRunStatus::Completed,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                    journal,
                ));
            }

            let completed_calls = calls
                .into_values()
                .map(ToolCallBuilder::complete)
                .collect::<Result<Vec<_>, _>>()?;
            request.messages.push(ModelMessage {
                role: "assistant".into(),
                content: json!({
                    "text": text,
                    "tool_calls": completed_calls.iter().map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments},
                        "provider_metadata": call.provider_metadata,
                    })).collect::<Vec<_>>()
                }),
            });

            if tool_calls.saturating_add(completed_calls.len() as u32) > self.limits.max_tool_calls
            {
                materialize_pending_steps(&mut step_queue, &mut request.messages);
                return self.fail_run(
                    &mut machine,
                    &mut journal,
                    TOOL_CALL_LIMIT_REASON,
                    assistant_text,
                    request.messages,
                    input_tokens,
                    output_tokens,
                    model_calls,
                    tool_calls,
                );
            }

            machine.transition(TurnStatus::RunningTool)?;
            self.emit_status(&machine, &mut journal);
            let mut tool_schedule = ToolSchedule::default();
            for call in completed_calls {
                self.observer.emit(AgentEvent::ToolProposed {
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                });
                machine.record_tool_call()?;
                tool_calls += 1;
                journal.append(AgentOperation::ToolCallStarted {
                    call_id: call.id.clone(),
                    tool: call.name.clone(),
                });
                let prepared = match serde_json::from_str(&call.arguments) {
                    Ok(arguments) => tokio::select! {
                        () = cancellation.cancelled() => {
                            materialize_cancelled_steps(&mut step_queue, &mut request.messages);
                            machine.transition(TurnStatus::Cancelled)?;
                            self.emit_status(&machine, &mut journal);
                            return Ok(result(
                                AgentRunStatus::Cancelled,
                                assistant_text,
                                request.messages,
                                input_tokens,
                                output_tokens,
                                model_calls,
                                tool_calls,
                                journal,
                            ));
                        }
                        () = tokio::time::sleep_until(turn_deadline) => {
                            materialize_pending_steps(&mut step_queue, &mut request.messages);
                            return self.fail_run(
                                &mut machine,
                                &mut journal,
                                TURN_ELAPSED_TIMEOUT_REASON,
                                assistant_text,
                                request.messages,
                                input_tokens,
                                output_tokens,
                                model_calls,
                                tool_calls,
                            );
                        }
                        prepared = self.executor.prepare(
                            &call.id,
                            &call.name,
                            arguments,
                            &cancellation,
                        ) => prepared,
                    },
                    Err(error) => PreparedAgentToolCall::resolved(AgentToolResult::Failed {
                        error: format!("tool arguments are not valid JSON: {error}"),
                    }),
                };
                let claims = if prepared.may_pause {
                    vec![ResourceClaim::global_exclusive()]
                } else {
                    prepared.claims.clone()
                };
                let fingerprint = format!("{}:{}", call.name, call.arguments);
                tool_schedule.push(
                    PendingToolCall {
                        call,
                        prepared,
                        fingerprint,
                    },
                    claims,
                );
            }

            while !tool_schedule.is_empty() {
                let wave = tool_schedule.take_ready_wave();

                let executions = wave.into_iter().map(|scheduled| async {
                    let pending = scheduled.value;
                    let outcome = self
                        .executor
                        .execute_prepared(
                            &pending.call.id,
                            &pending.call.name,
                            pending.prepared.clone(),
                            &cancellation,
                        )
                        .await;
                    (pending, outcome)
                });
                let outcomes = tokio::select! {
                    () = cancellation.cancelled() => {
                        materialize_cancelled_steps(&mut step_queue, &mut request.messages);
                        machine.transition(TurnStatus::Cancelled)?;
                        self.emit_status(&machine, &mut journal);
                        return Ok(result(
                            AgentRunStatus::Cancelled,
                            assistant_text,
                            request.messages,
                            input_tokens,
                            output_tokens,
                            model_calls,
                            tool_calls,
                            journal,
                        ));
                    }
                    () = tokio::time::sleep_until(turn_deadline) => {
                        materialize_pending_steps(&mut step_queue, &mut request.messages);
                        return self.fail_run(
                            &mut machine,
                            &mut journal,
                            TURN_ELAPSED_TIMEOUT_REASON,
                            assistant_text,
                            request.messages,
                            input_tokens,
                            output_tokens,
                            model_calls,
                            tool_calls,
                        );
                    }
                    outcomes = join_all(executions) => outcomes,
                };

                let mut paused = None;
                let mut repeated_failure = false;
                for (pending, tool_result) in outcomes {
                    let call = pending.call;
                    let fingerprint = pending.fingerprint;
                    if !self.executor.owns_tool_lifecycle(&call.name) {
                        self.observer.emit(AgentEvent::ToolFinished {
                            call_id: call.id.clone(),
                            tool: call.name.clone(),
                            success: !matches!(&tool_result, AgentToolResult::Failed { .. }),
                        });
                    }
                    match tool_result {
                        AgentToolResult::Completed { value } => {
                            failures.remove(&fingerprint);
                            enqueue_tool_result(
                                &mut step_queue,
                                tool_message(&call.id, &call.name, value),
                            );
                        }
                        AgentToolResult::ContextChanged {
                            retired_system_content,
                            value,
                            tools,
                            system_messages,
                        } => {
                            failures.remove(&fingerprint);
                            request.tools = tools;
                            request.messages.retain(|message| {
                                message.role != "system"
                                    || message.content != retired_system_content
                            });
                            request.messages.splice(0..0, system_messages);
                            enqueue_tool_result(
                                &mut step_queue,
                                tool_message(&call.id, &call.name, value),
                            );
                        }
                        AgentToolResult::DiscoveredTools { value, tools } => {
                            failures.remove(&fingerprint);
                            for discovered in tools {
                                if let Some(existing) = request
                                    .tools
                                    .iter_mut()
                                    .find(|existing| existing.name == discovered.name)
                                {
                                    *existing = discovered;
                                } else {
                                    request.tools.push(discovered);
                                }
                            }
                            enqueue_tool_result(
                                &mut step_queue,
                                tool_message(&call.id, &call.name, value),
                            );
                        }
                        AgentToolResult::AwaitingInput { detail } => {
                            paused.get_or_insert(AgentRunStatus::AwaitingInput { detail });
                        }
                        AgentToolResult::AwaitingApproval { detail } => {
                            paused.get_or_insert(AgentRunStatus::AwaitingApproval { detail });
                        }
                        AgentToolResult::Failed { error } => {
                            let count = failures.entry(fingerprint).or_default();
                            *count += 1;
                            enqueue_tool_result(
                                &mut step_queue,
                                tool_message(
                                    &call.id,
                                    &call.name,
                                    json!({"error": error, "retryable": *count < self.repeated_failure_limit}),
                                ),
                            );
                            repeated_failure |= *count >= self.repeated_failure_limit;
                        }
                    }
                }

                if let Some(status) = paused {
                    defer_pending_tool_calls(&mut tool_schedule, &mut step_queue);
                    materialize_pending_steps(&mut step_queue, &mut request.messages);
                    machine.transition(match &status {
                        AgentRunStatus::AwaitingInput { .. } => TurnStatus::AwaitingInput,
                        AgentRunStatus::AwaitingApproval { .. } => TurnStatus::AwaitingApproval,
                        _ => unreachable!("only pausing statuses are stored"),
                    })?;
                    self.emit_status(&machine, &mut journal);
                    return Ok(result(
                        status,
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                        journal,
                    ));
                }
                if repeated_failure {
                    materialize_pending_steps(&mut step_queue, &mut request.messages);
                    machine.transition(TurnStatus::Failed)?;
                    self.emit_status(&machine, &mut journal);
                    return Ok(result(
                        AgentRunStatus::Failed {
                            reason: "repeated identical tool failure".into(),
                        },
                        assistant_text,
                        request.messages,
                        input_tokens,
                        output_tokens,
                        model_calls,
                        tool_calls,
                        journal,
                    ));
                }
            }
            machine.transition(TurnStatus::CallingModel)?;
            self.emit_status(&machine, &mut journal);
            step_queue
                .admit(
                    StepRequest::continuation("tool_results_ready"),
                    true,
                    QueuePosition::Tail,
                )
                .expect("a tool continuation is admitted only while its Turn is active");
        }
    }

    fn emit_status(&self, machine: &TurnMachine, journal: &mut AgentJournal) {
        journal.append(AgentOperation::StatusChanged {
            status: machine.status().clone(),
        });
        self.observer.emit(AgentEvent::Status {
            status: machine.status().clone(),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn fail_run(
        &self,
        machine: &mut TurnMachine,
        journal: &mut AgentJournal,
        reason: &str,
        assistant_text: String,
        messages: Vec<ModelMessage>,
        input_tokens: u64,
        output_tokens: u64,
        model_calls: u32,
        tool_calls: u32,
    ) -> Result<AgentRunResult, AgentError> {
        machine.transition(TurnStatus::Failed)?;
        self.emit_status(machine, journal);
        Ok(result(
            AgentRunStatus::Failed {
                reason: reason.into(),
            },
            assistant_text,
            messages,
            input_tokens,
            output_tokens,
            model_calls,
            tool_calls,
            std::mem::take(journal),
        ))
    }

    async fn stream_with_retry(
        &self,
        request: &mut ModelRequest,
        cancellation: &CancellationToken,
    ) -> Result<s_code_model_gateway::ModelStream, AgentError> {
        let mut attempt = 0;
        let mut context_compacted = false;
        loop {
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(AgentError::Cancelled),
                response = self.provider.stream(request.clone()) => response,
            };
            match response {
                Ok(stream) => return Ok(stream),
                Err(GatewayError::ContextOverflow(message)) if !context_compacted => {
                    let Some(compaction) = compact_overflow_request(request) else {
                        return Err(AgentError::Gateway(GatewayError::ContextOverflow(message)));
                    };
                    context_compacted = true;
                    self.observer.emit(AgentEvent::ContextCompacted {
                        omitted_messages: compaction.omitted_messages,
                        truncated_messages: compaction.truncated_messages,
                        estimated_tokens: compaction.estimated_tokens,
                    });
                }
                Err(error)
                    if attempt < self.max_provider_retries && error.fallback_reason().is_some() =>
                {
                    attempt += 1;
                    tokio::select! {
                        () = cancellation.cancelled() => return Err(AgentError::Cancelled),
                        () = tokio::time::sleep(std::time::Duration::from_millis(50 * u64::from(attempt))) => {}
                    }
                }
                Err(error) => return Err(AgentError::Gateway(error)),
            }
        }
    }
}

struct OverflowCompaction {
    omitted_messages: u32,
    truncated_messages: u32,
    estimated_tokens: u64,
}

fn compact_overflow_request(request: &mut ModelRequest) -> Option<OverflowCompaction> {
    let conversation = request
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| ConversationMessage {
            id: format!("overflow-{index}"),
            role: message.role.clone(),
            content: message.content.clone(),
        })
        .collect::<Vec<_>>();
    let before = estimate_conversation_tokens(&conversation);
    if before < 256 {
        return None;
    }
    let target = before.saturating_mul(2).checked_div(3).unwrap_or(before);
    let packed = pack_conversation_history(
        conversation,
        target.max(128),
        target.saturating_div(2).max(64),
        target.saturating_div(4).max(64),
    );
    let after = packed.estimated_tokens
        + packed
            .compaction
            .as_ref()
            .map_or(0, |summary| summary.content.chars().count().div_ceil(4));
    if after >= before || (packed.omitted_ids.is_empty() && packed.truncated_ids.is_empty()) {
        return None;
    }
    let mut messages =
        Vec::with_capacity(packed.messages.len() + usize::from(packed.compaction.is_some()));
    if let Some(summary) = packed.compaction {
        messages.push(ModelMessage {
            role: "system".into(),
            content: json!({
                "context_overflow_summary": summary.content,
                "trust": "derived-untrusted",
            }),
        });
    }
    messages.extend(packed.messages.into_iter().map(|message| ModelMessage {
        role: message.role,
        content: message.content,
    }));
    request.messages = messages;
    Some(OverflowCompaction {
        omitted_messages: u32::try_from(packed.omitted_ids.len()).unwrap_or(u32::MAX),
        truncated_messages: u32::try_from(packed.truncated_ids.len()).unwrap_or(u32::MAX),
        estimated_tokens: u64::try_from(after).unwrap_or(u64::MAX),
    })
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Turn(#[from] TurnError),
    #[error(transparent)]
    Gateway(#[from] GatewayError),
    #[error("invalid model tool call: {0}")]
    InvalidToolCall(String),
    #[error("invalid agent request: {0}")]
    InvalidRequest(String),
    #[error("agent run cancelled")]
    Cancelled,
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    provider_metadata: Option<Value>,
}

struct CompletedToolCall {
    id: String,
    name: String,
    arguments: String,
    provider_metadata: Option<Value>,
}

struct PendingToolCall {
    call: CompletedToolCall,
    prepared: PreparedAgentToolCall,
    fingerprint: String,
}

impl ToolCallBuilder {
    fn complete(self) -> Result<CompletedToolCall, AgentError> {
        Ok(CompletedToolCall {
            id: self
                .id
                .ok_or_else(|| AgentError::InvalidToolCall("missing call id".into()))?,
            name: self
                .name
                .ok_or_else(|| AgentError::InvalidToolCall("missing tool name".into()))?,
            arguments: self.arguments,
            provider_metadata: self.provider_metadata,
        })
    }
}

fn tool_message(id: &str, name: &str, value: Value) -> ModelMessage {
    ModelMessage {
        role: "tool".into(),
        content: json!({"tool_call_id": id, "name": name, "result": value}),
    }
}

fn latest_failed_verifier_result(messages: &[ModelMessage]) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| {
            message.role == "tool" && message.content["name"].as_str() == Some("run_command")
        })
        .and_then(|(index, message)| {
            let result = &message.content["result"];
            let failed = result
                .get("exit_code")
                .and_then(Value::as_i64)
                .is_some_and(|exit_code| exit_code != 0)
                || result.get("error").is_some();
            failed.then_some(index)
        })
}

fn compact_superseded_tool_history(messages: &mut Vec<ModelMessage>) -> usize {
    let tool_messages = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (message.role == "tool")
                .then(|| {
                    message
                        .content
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .map(|id| (index, id.to_owned()))
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    if tool_messages.len() <= RECENT_DETAILED_TOOL_RESULTS {
        return 0;
    }

    // Keep one unresolved verifier failure visible even after the model reads
    // several files to diagnose it. A later successful verifier supersedes it.
    // Without this pin, the concrete failure disappears at exactly the point
    // the model has gathered enough source context to make a fix.
    let pinned_failure = latest_failed_verifier_result(messages);
    let older = tool_messages[..tool_messages.len() - RECENT_DETAILED_TOOL_RESULTS]
        .iter()
        .filter(|(index, _)| Some(*index) != pinned_failure)
        .collect::<Vec<_>>();
    let older_ids = older
        .iter()
        .map(|(_, id)| (*id).clone())
        .collect::<HashSet<_>>();
    let mut newly_compacted = 0;
    for (index, id) in older {
        let message = &mut messages[*index];
        let already_compacted = message.content["result"]["history_compacted"]
            .as_bool()
            .unwrap_or(false);
        let name = message.content["name"]
            .as_str()
            .unwrap_or("tool")
            .to_owned();
        message.content = json!({
            "tool_call_id": id,
            "name": name,
            "result": {
                "history_compacted": true,
                "reason": "superseded tool detail omitted; the workspace and recent tool results are authoritative"
            }
        });
        newly_compacted += usize::from(!already_compacted);
    }

    for message in messages
        .iter_mut()
        .filter(|message| message.role == "assistant")
    {
        let Some(calls) = message
            .content
            .get_mut("tool_calls")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        for call in calls {
            let Some(id) = call.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !older_ids.contains(id) {
                continue;
            }
            if let Some(function) = call.get_mut("function").and_then(Value::as_object_mut) {
                function.insert(
                    "arguments".into(),
                    Value::String("{\"history_compacted\":true}".into()),
                );
            }
            if let Some(object) = call.as_object_mut() {
                object.insert("provider_metadata".into(), Value::Null);
            }
        }
    }

    if newly_compacted > 0
        && !messages.iter().any(|message| {
            message
                .content
                .get("tool_history_compaction")
                .and_then(Value::as_bool)
                == Some(true)
        })
    {
        let insertion = messages
            .iter()
            .take_while(|message| message.role == "system")
            .count();
        messages.insert(
            insertion,
            ModelMessage {
                role: "system".into(),
                content: json!({
                    "tool_history_compaction": true,
                    "instruction": "Older completed tool payloads were compacted to control latency and token use. Treat the current workspace and recent tool results as authoritative; re-read a file before editing if its current state is needed."
                }),
            },
        );
    }
    newly_compacted
}

fn initial_step_queue(messages: &mut Vec<ModelMessage>) -> StepRequestQueue {
    let mut queue = StepRequestQueue::default();
    let Some(message) = messages.pop() else {
        queue
            .admit(
                StepRequest::new(
                    "initial_model_step",
                    Vec::new(),
                    StepRequestOptions {
                        admission: StepAdmission::ActiveOrNewTurn,
                        mergeable: false,
                        turn_scoped: true,
                    },
                ),
                false,
                QueuePosition::Tail,
            )
            .expect("an initial model step can create a Turn");
        return queue;
    };
    let is_user_input = message.role == "user";
    queue
        .admit(
            StepRequest::new(
                if is_user_input {
                    "user_input"
                } else {
                    "resume_context"
                },
                vec![message],
                StepRequestOptions {
                    admission: if is_user_input {
                        StepAdmission::NewTurn
                    } else {
                        StepAdmission::ActiveOrNewTurn
                    },
                    mergeable: false,
                    turn_scoped: true,
                },
            ),
            false,
            QueuePosition::Tail,
        )
        .expect("initial input must target a new Turn");
    queue
}

fn enqueue_tool_result(queue: &mut StepRequestQueue, message: ModelMessage) {
    queue
        .admit(
            StepRequest::new(
                "tool_result",
                vec![message],
                StepRequestOptions {
                    admission: StepAdmission::ActiveTurnOnly,
                    mergeable: true,
                    turn_scoped: false,
                },
            ),
            true,
            QueuePosition::Tail,
        )
        .expect("a tool result belongs to its active Turn");
}

fn enqueue_empty_model_retry(queue: &mut StepRequestQueue) {
    queue
        .admit(
            StepRequest::new(
                "empty_model_retry",
                vec![ModelMessage {
                    role: "user".into(),
                    content: Value::String(
                        "[Harness retry] The previous response was empty. Continue the task: either use the tools needed to make progress or provide a concrete final answer."
                            .into(),
                    ),
                }],
                StepRequestOptions {
                    admission: StepAdmission::ActiveTurnOnly,
                    mergeable: false,
                    turn_scoped: true,
                },
            ),
            true,
            QueuePosition::Tail,
        )
        .expect("an empty-response retry belongs to its active Turn");
}

fn enqueue_incomplete_model_retry(queue: &mut StepRequestQueue) {
    queue
        .admit(
            StepRequest::new(
                "incomplete_model_retry",
                vec![ModelMessage {
                    role: "user".into(),
                    content: Value::String(
                        "[Harness retry] The previous response was truncated by the output limit. Continue directly from it without repeating text, verify the result, and then conclude."
                            .into(),
                    ),
                }],
                StepRequestOptions {
                    admission: StepAdmission::ActiveTurnOnly,
                    mergeable: false,
                    turn_scoped: true,
                },
            ),
            true,
            QueuePosition::Tail,
        )
        .expect("an incomplete-response retry belongs to its active Turn");
}

fn enqueue_model_stream_retry(queue: &mut StepRequestQueue) {
    queue
        .admit(
            StepRequest::new(
                "model_stream_retry",
                Vec::new(),
                StepRequestOptions {
                    admission: StepAdmission::ActiveTurnOnly,
                    mergeable: false,
                    turn_scoped: true,
                },
            ),
            true,
            QueuePosition::Tail,
        )
        .expect("a model stream retry belongs to its active Turn");
}

fn drain_external_steps(
    ingress: &mut Option<mpsc::Receiver<StepRequest>>,
    queue: &mut StepRequestQueue,
) {
    let Some(receiver) = ingress.as_mut() else {
        return;
    };
    while let Ok(request) = receiver.try_recv() {
        if request.admission.target(true) == Ok(AdmissionTarget::ActiveTurn) {
            queue.enqueue(request, QueuePosition::Tail);
        }
    }
}

async fn receive_external_step(ingress: &mut Option<mpsc::Receiver<StepRequest>>) -> StepRequest {
    loop {
        match ingress {
            Some(receiver) => match receiver.recv().await {
                Some(request) => return request,
                None => *ingress = None,
            },
            None => futures_util::future::pending().await,
        }
    }
}

async fn close_and_drain_external_steps(
    ingress: &mut Option<mpsc::Receiver<StepRequest>>,
    queue: &mut StepRequestQueue,
) {
    let Some(receiver) = ingress.as_mut() else {
        return;
    };
    receiver.close();
    while let Some(request) = receiver.recv().await {
        if request.admission.target(true) == Ok(AdmissionTarget::ActiveTurn) {
            queue.enqueue(request, QueuePosition::Tail);
        }
    }
}

fn materialize_pending_steps(queue: &mut StepRequestQueue, messages: &mut Vec<ModelMessage>) {
    messages.extend(queue.drain_materialized_messages());
}

fn materialize_cancelled_steps(queue: &mut StepRequestQueue, messages: &mut Vec<ModelMessage>) {
    queue.abort_turn_scoped();
    materialize_pending_steps(queue, messages);
}

fn preserve_partial_model_text(
    text: &mut String,
    assistant_text: &mut String,
    messages: &mut Vec<ModelMessage>,
    journal: &mut AgentJournal,
) {
    if text.is_empty() {
        return;
    }
    journal.append(AgentOperation::TextAppended { text: text.clone() });
    assistant_text.push_str(text);
    messages.push(ModelMessage {
        role: "assistant".into(),
        content: Value::String(std::mem::take(text)),
    });
}

fn defer_pending_tool_calls(
    schedule: &mut ToolSchedule<PendingToolCall>,
    queue: &mut StepRequestQueue,
) {
    for scheduled in schedule.drain() {
        let call = scheduled.value.call;
        enqueue_tool_result(
            queue,
            tool_message(
                &call.id,
                &call.name,
                json!({
                    "error": "tool call deferred while another call awaits interaction",
                    "retryable": true,
                    "deferred": true,
                }),
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn result(
    status: AgentRunStatus,
    assistant_text: String,
    messages: Vec<ModelMessage>,
    input_tokens: u64,
    output_tokens: u64,
    model_calls: u32,
    tool_calls: u32,
    journal: AgentJournal,
) -> AgentRunResult {
    debug_assert_eq!(journal.projection.assistant_text, assistant_text);
    debug_assert_eq!(journal.projection.input_tokens, input_tokens);
    debug_assert_eq!(journal.projection.output_tokens, output_tokens);
    debug_assert_eq!(journal.projection.model_calls, model_calls);
    debug_assert_eq!(journal.projection.tool_calls, tool_calls);
    AgentRunResult {
        status,
        assistant_text,
        messages,
        input_tokens,
        output_tokens,
        model_calls,
        tool_calls,
        ops: journal.operations,
    }
}

fn valid(from: &TurnStatus, to: &TurnStatus) -> bool {
    use TurnStatus::*;
    if matches!(to, Cancelled) {
        return matches!(
            from,
            Idle | PreparingContext | CallingModel | AwaitingInput | AwaitingApproval | RunningTool
        );
    }
    matches!(
        (from, to),
        (Idle, PreparingContext)
            | (PreparingContext, CallingModel | Failed)
            | (
                CallingModel,
                AwaitingInput | AwaitingApproval | RunningTool | Completed | Failed
            )
            | (AwaitingInput, RunningTool | CallingModel | Failed)
            | (AwaitingApproval, RunningTool | CallingModel | Failed)
            | (
                RunningTool,
                AwaitingInput | AwaitingApproval | CallingModel | Failed
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, body::Body, extract::State, http::StatusCode, response::Response,
        routing::post,
    };
    use s_code_model_gateway::{
        AnthropicMessages, CredentialProvider, GeminiGenerateContent, OpenAiCompatible,
    };
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    struct FakeProvider {
        responses: Mutex<VecDeque<Vec<ModelEvent>>>,
    }

    struct OverflowOnceProvider {
        calls: AtomicUsize,
        requests: Mutex<Vec<ModelRequest>>,
    }

    struct IngressProvider {
        calls: AtomicUsize,
        requests: Mutex<Vec<ModelRequest>>,
    }

    struct CompletedWithoutEofProvider;

    struct IdleAfterTextProvider;

    struct IdleOnceProvider {
        calls: AtomicUsize,
    }

    struct LivenessOnlyIdleOnceProvider {
        calls: AtomicUsize,
    }

    struct StreamErrorOnceProvider {
        calls: AtomicUsize,
    }

    struct NeverStartsProvider;

    struct NeverIdleProvider;

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<AgentEvent>>,
    }

    impl AgentObserver for RecordingObserver {
        fn emit(&self, event: AgentEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn older_tool_payloads_are_compacted_while_recent_results_remain_detailed() {
        let mut messages = vec![ModelMessage {
            role: "system".into(),
            content: Value::String("system".into()),
        }];
        for index in 0..8 {
            messages.push(ModelMessage {
                role: "assistant".into(),
                content: json!({
                    "tool_calls": [{
                        "id": format!("call-{index}"),
                        "function": {
                            "name": "read_file",
                            "arguments": format!("{{\"path\":\"large-{index}.txt\"}}")
                        }
                    }]
                }),
            });
            messages.push(tool_message(
                &format!("call-{index}"),
                "read_file",
                json!({"content": "large detailed result"}),
            ));
        }

        assert_eq!(compact_superseded_tool_history(&mut messages), 2);
        let old_result = messages
            .iter()
            .find(|message| message.content["tool_call_id"] == "call-0")
            .unwrap();
        assert_eq!(old_result.content["result"]["history_compacted"], true);
        let recent_result = messages
            .iter()
            .find(|message| message.content["tool_call_id"] == "call-7")
            .unwrap();
        assert_eq!(
            recent_result.content["result"]["content"],
            "large detailed result"
        );
        let old_call = messages
            .iter()
            .find(|message| {
                message.content["tool_calls"][0]["id"] == Value::String("call-0".into())
            })
            .unwrap();
        assert_eq!(
            old_call.content["tool_calls"][0]["function"]["arguments"],
            "{\"history_compacted\":true}"
        );
        assert!(
            messages
                .iter()
                .any(|message| { message.content["tool_history_compaction"] == Value::Bool(true) })
        );
        assert_eq!(compact_superseded_tool_history(&mut messages), 0);
    }

    #[test]
    fn latest_failed_verifier_survives_read_history_compaction() {
        let mut messages = vec![ModelMessage {
            role: "system".into(),
            content: Value::String("system".into()),
        }];
        messages.push(ModelMessage {
            role: "assistant".into(),
            content: json!({
                "tool_calls": [{
                    "id": "verify-failed",
                    "function": {"name": "run_command", "arguments": "{\"program\":\"test\"}"}
                }]
            }),
        });
        messages.push(tool_message(
            "verify-failed",
            "run_command",
            json!({"exit_code": 1, "stderr": "one focused assertion failed"}),
        ));
        for index in 0..8 {
            messages.push(ModelMessage {
                role: "assistant".into(),
                content: json!({
                    "tool_calls": [{
                        "id": format!("read-{index}"),
                        "function": {"name": "read_file", "arguments": "{}"}
                    }]
                }),
            });
            messages.push(tool_message(
                &format!("read-{index}"),
                "read_file",
                json!({"content": "source"}),
            ));
        }

        assert_eq!(compact_superseded_tool_history(&mut messages), 2);
        let failure = messages
            .iter()
            .find(|message| message.content["tool_call_id"] == "verify-failed")
            .unwrap();
        assert_eq!(failure.content["result"]["exit_code"], 1);
        assert_eq!(
            failure.content["result"]["stderr"],
            "one focused assertion failed"
        );
        let old_read = messages
            .iter()
            .find(|message| message.content["tool_call_id"] == "read-0")
            .unwrap();
        assert_eq!(old_read.content["result"]["history_compacted"], true);
    }

    #[test]
    fn successful_verifier_supersedes_an_older_failure() {
        let mut messages = Vec::new();
        messages.push(tool_message(
            "verify-failed",
            "run_command",
            json!({"exit_code": 1, "stderr": "failed"}),
        ));
        messages.push(tool_message(
            "verify-passed",
            "run_command",
            json!({"exit_code": 0, "stdout": "passed"}),
        ));
        for index in 0..7 {
            messages.push(tool_message(
                &format!("read-{index}"),
                "read_file",
                json!({"content": "source"}),
            ));
        }

        assert_eq!(compact_superseded_tool_history(&mut messages), 3);
        let failure = messages
            .iter()
            .find(|message| message.content["tool_call_id"] == "verify-failed")
            .unwrap();
        assert_eq!(failure.content["result"]["history_compacted"], true);
    }

    #[async_trait]
    impl ModelProvider for FakeProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            let events = self.responses.lock().unwrap().pop_front().unwrap();
            Ok(Box::pin(futures_util::stream::iter(
                events.into_iter().map(Ok),
            )))
        }
    }

    #[async_trait]
    impl ModelProvider for OverflowOnceProvider {
        async fn stream(
            &self,
            request: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            self.requests.lock().unwrap().push(request);
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(GatewayError::ContextOverflow("too many tokens".into()));
            }
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "recovered".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ModelProvider for IngressProvider {
        async fn stream(
            &self,
            request: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            self.requests.lock().unwrap().push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                let delayed = futures_util::stream::once(async {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    Ok(ModelEvent::TextDelta {
                        text: "first".into(),
                    })
                });
                return Ok(Box::pin(delayed.chain(futures_util::stream::iter(vec![
                    Ok(ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    }),
                ]))));
            }
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "second".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ModelProvider for CompletedWithoutEofProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            let events = futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "complete".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ]);
            Ok(Box::pin(events.chain(futures_util::stream::pending())))
        }
    }

    #[async_trait]
    impl ModelProvider for IdleAfterTextProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            let text = futures_util::stream::once(async {
                Ok(ModelEvent::TextDelta {
                    text: "partial".into(),
                })
            });
            Ok(Box::pin(text.chain(futures_util::stream::pending())))
        }
    }

    #[async_trait]
    impl ModelProvider for IdleOnceProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(Box::pin(futures_util::stream::pending()));
            }
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "recovered".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ModelProvider for LivenessOnlyIdleOnceProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                let liveness = futures_util::stream::iter(vec![Ok(ModelEvent::TextDelta {
                    text: String::new(),
                })]);
                return Ok(Box::pin(liveness.chain(futures_util::stream::pending())));
            }
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "recovered".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ModelProvider for StreamErrorOnceProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(Box::pin(futures_util::stream::iter(vec![Err(
                    GatewayError::Provider("connection reset".into()),
                )])));
            }
            Ok(Box::pin(futures_util::stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    text: "recovered".into(),
                }),
                Ok(ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }),
            ])))
        }
    }

    #[async_trait]
    impl ModelProvider for NeverStartsProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            futures_util::future::pending().await
        }
    }

    #[async_trait]
    impl ModelProvider for NeverIdleProvider {
        async fn stream(
            &self,
            _: ModelRequest,
        ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
            Ok(Box::pin(futures_util::stream::unfold((), |_| async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Some((
                    Ok(ModelEvent::ReasoningSummaryDelta { text: ".".into() }),
                    (),
                ))
            })))
        }
    }

    struct FakeExecutor {
        outcome: AgentToolResult,
    }

    struct DiscoveryExecutor;

    struct ConcurrentReadExecutor {
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    struct PausingExecutor {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl AgentToolExecutor for ConcurrentReadExecutor {
        fn may_pause(&self, _: &str, _: &Value) -> bool {
            false
        }

        fn resource_claims(&self, _: &str, arguments: &Value) -> Vec<ResourceClaim> {
            vec![
                ResourceClaim::workspace(
                    arguments["path"].as_str().unwrap(),
                    tool::ResourceMode::Read,
                    false,
                )
                .unwrap(),
            ]
        }

        async fn execute(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: &CancellationToken,
        ) -> AgentToolResult {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            AgentToolResult::Completed {
                value: json!({"read": true}),
            }
        }
    }

    #[async_trait]
    impl AgentToolExecutor for PausingExecutor {
        async fn execute(
            &self,
            call_id: &str,
            tool: &str,
            _: Value,
            _: &CancellationToken,
        ) -> AgentToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            AgentToolResult::AwaitingApproval {
                detail: json!({"call_id": call_id, "tool": tool}),
            }
        }
    }

    #[async_trait]
    impl AgentToolExecutor for DiscoveryExecutor {
        async fn execute(
            &self,
            _: &str,
            tool: &str,
            _: Value,
            _: &CancellationToken,
        ) -> AgentToolResult {
            match tool {
                "tool_search" => AgentToolResult::DiscoveredTools {
                    value: json!({
                        "matched": 1,
                        "tools": [{"name": "mcp.docs.lookup", "description": "Look up docs"}],
                    }),
                    tools: vec![ToolDefinition {
                        name: "mcp.docs.lookup".into(),
                        description: "Look up docs".into(),
                        parameters: json!({
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        }),
                    }],
                },
                "mcp.docs.lookup" => AgentToolResult::Completed {
                    value: json!({"result": "bounded documentation"}),
                },
                _ => AgentToolResult::Failed {
                    error: "unexpected tool".into(),
                },
            }
        }
    }

    struct IntegrationCredentials;

    #[async_trait]
    impl CredentialProvider for IntegrationCredentials {
        async fn resolve(&self, handle: &str) -> Result<String, GatewayError> {
            assert_eq!(handle, "integration-provider");
            Ok("integration-token".into())
        }
    }

    #[async_trait]
    impl AgentToolExecutor for FakeExecutor {
        async fn execute(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: &CancellationToken,
        ) -> AgentToolResult {
            self.outcome.clone()
        }
    }

    fn request() -> AgentRunRequest {
        AgentRunRequest {
            model: "fake".into(),
            temperature: 0.0,
            messages: vec![ModelMessage {
                role: "user".into(),
                content: Value::String("inspect".into()),
            }],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "read".into(),
                parameters: json!({"type":"object"}),
            }],
            max_output_tokens: 100,
            routing: None,
        }
    }

    fn tool_response() -> Vec<ModelEvent> {
        vec![
            ModelEvent::ToolCallDelta {
                index: 0,
                id: Some("call_1".into()),
                name: Some("read_file".into()),
                arguments_delta: "{\"path\":\"a.txt\"}".into(),
                provider_metadata: Some(json!("opaque-signature")),
            },
            ModelEvent::Completed {
                finish_reason: Some("tool_calls".into()),
            },
        ]
    }

    fn two_read_tool_response() -> Vec<ModelEvent> {
        vec![
            ModelEvent::ToolCallDelta {
                index: 0,
                id: Some("call_a".into()),
                name: Some("read_file".into()),
                arguments_delta: "{\"path\":\"a.txt\"}".into(),
                provider_metadata: None,
            },
            ModelEvent::ToolCallDelta {
                index: 1,
                id: Some("call_b".into()),
                name: Some("read_file".into()),
                arguments_delta: "{\"path\":\"b.txt\"}".into(),
                provider_metadata: None,
            },
            ModelEvent::Completed {
                finish_reason: Some("tool_calls".into()),
            },
        ]
    }

    fn malformed_tool_response() -> Vec<ModelEvent> {
        vec![
            ModelEvent::ToolCallDelta {
                index: 0,
                id: Some("call_bad".into()),
                name: Some("read_file".into()),
                arguments_delta: "{\"path\":".into(),
                provider_metadata: None,
            },
            ModelEvent::Completed {
                finish_reason: Some("tool_calls".into()),
            },
        ]
    }
    #[test]
    fn happy_path_is_explicit() {
        let mut m = TurnMachine::new(TurnLimits::default());
        m.transition(TurnStatus::PreparingContext).unwrap();
        m.transition(TurnStatus::CallingModel).unwrap();
        m.transition(TurnStatus::RunningTool).unwrap();
        m.transition(TurnStatus::CallingModel).unwrap();
        m.transition(TurnStatus::Completed).unwrap();
    }
    #[test]
    fn completed_turn_cannot_restart() {
        let mut m = TurnMachine::new(TurnLimits::default());
        m.transition(TurnStatus::PreparingContext).unwrap();
        m.transition(TurnStatus::CallingModel).unwrap();
        m.transition(TurnStatus::Completed).unwrap();
        assert!(matches!(
            m.transition(TurnStatus::CallingModel),
            Err(TurnError::InvalidTransition { .. })
        ));
    }
    #[test]
    fn budget_fails_closed() {
        let mut m = TurnMachine::new(TurnLimits {
            max_model_calls: 1,
            ..Default::default()
        });
        m.record_model_call(1).unwrap();
        assert_eq!(
            m.record_model_call(1),
            Err(TurnError::LimitExceeded("model_calls"))
        );
    }

    #[tokio::test]
    async fn active_step_ingress_is_materialized_before_the_turn_completes() {
        let provider = Arc::new(IngressProvider {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let runner = AgentRunner::new(
            provider.clone(),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        );
        let (sender, receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            sender
                .send(StepRequest::new(
                    "steer_input",
                    vec![ModelMessage {
                        role: "user".into(),
                        content: json!("use the safer approach"),
                    }],
                    StepRequestOptions {
                        admission: StepAdmission::ActiveTurnOnly,
                        mergeable: true,
                        turn_scoped: false,
                    },
                ))
                .await
                .unwrap();
        });

        let result = runner
            .run_with_step_ingress(request(), CancellationToken::new(), receiver)
            .await
            .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "firstsecond");
        assert_eq!(result.model_calls, 2);
        let requests = provider.requests.lock().unwrap();
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.content == json!("use the safer approach"))
        );
    }

    #[test]
    fn checkpoint_decode_migrates_legacy_and_rejects_unknown_versions() {
        let legacy = AgentRunResult {
            status: AgentRunStatus::AwaitingApproval {
                detail: json!({"model_call_id":"call-1"}),
            },
            assistant_text: String::new(),
            messages: Vec::new(),
            input_tokens: 3,
            output_tokens: 2,
            model_calls: 1,
            tool_calls: 1,
            ops: Vec::new(),
        };
        let migrated = AgentCheckpoint::decode(serde_json::to_value(&legacy).unwrap()).unwrap();
        assert_eq!(migrated.schema_version, AGENT_CHECKPOINT_SCHEMA_VERSION);
        assert_eq!(migrated.result.status, legacy.status);

        let unknown = serde_json::json!({
            "schema_version": 99,
            "result": legacy,
        });
        assert!(matches!(
            AgentCheckpoint::decode(unknown),
            Err(AgentCheckpointError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn agent_operation_replay_is_pure_versioned_and_contiguous() {
        let operations = vec![
            AgentOp {
                schema_version: AGENT_OP_SCHEMA_VERSION,
                sequence: 1,
                operation: AgentOperation::ModelCallStarted,
            },
            AgentOp {
                schema_version: AGENT_OP_SCHEMA_VERSION,
                sequence: 2,
                operation: AgentOperation::TextAppended {
                    text: "hello".into(),
                },
            },
            AgentOp {
                schema_version: AGENT_OP_SCHEMA_VERSION,
                sequence: 3,
                operation: AgentOperation::UsageAdded {
                    input_tokens: 5,
                    output_tokens: 2,
                },
            },
        ];
        let first = replay_agent_ops(&operations).unwrap();
        let second = replay_agent_ops(&operations).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.assistant_text, "hello");
        assert_eq!(first.model_calls, 1);
        assert_eq!(first.input_tokens, 5);
        let mut gap = operations;
        gap[2].sequence = 4;
        assert_eq!(
            replay_agent_ops(&gap),
            Err(AgentOpError::SequenceGap {
                expected: 3,
                received: 4,
            })
        );
        gap[2].sequence = 3;
        gap[2].schema_version = 99;
        assert_eq!(
            replay_agent_ops(&gap),
            Err(AgentOpError::UnsupportedVersion(99))
        );
    }

    #[tokio::test]
    async fn context_overflow_compacts_once_and_retries_the_same_logical_step() {
        let provider = Arc::new(OverflowOnceProvider {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        });
        let mut request = request();
        request.messages = (0..20)
            .map(|index| ModelMessage {
                role: if index % 2 == 0 {
                    "user".into()
                } else {
                    "assistant".into()
                },
                content: json!(format!("message-{index} {}", "x".repeat(4_000))),
            })
            .collect();
        let before = request.messages.clone();
        let runner = AgentRunner::new(
            provider.clone(),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        );

        let result = runner.run(request, CancellationToken::new()).await.unwrap();

        assert!(matches!(result.status, AgentRunStatus::Completed));
        assert_eq!(result.assistant_text, "recovered");
        assert_eq!(result.model_calls, 1);
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].messages.len() < before.len());
        assert_eq!(
            requests[1].messages[0].content["trust"],
            "derived-untrusted"
        );
    }

    #[tokio::test]
    async fn model_tool_model_loop_completes() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([
                tool_response(),
                vec![
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::Usage {
                        input_tokens: 10,
                        output_tokens: 2,
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::Completed {
                value: json!({"content":"hello"}),
            },
        });
        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.tool_calls, 1);
        assert_eq!(result.assistant_text, "done");
        assert!(result.messages.iter().any(|message| message.role == "tool"));
        assert!(result.messages.iter().any(|message| {
            message.role == "assistant"
                && message.content["tool_calls"][0]["provider_metadata"]
                    == json!("opaque-signature")
        }));
    }

    #[tokio::test]
    async fn malformed_tool_json_is_returned_to_the_model_without_aborting_the_turn() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([
                malformed_tool_response(),
                vec![
                    ModelEvent::TextDelta {
                        text: "recovered".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let executor = Arc::new(PausingExecutor {
            calls: AtomicUsize::new(0),
        });
        let result = AgentRunner::new(provider, executor.clone(), TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "recovered");
        assert_eq!((result.model_calls, result.tool_calls), (2, 1));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
        assert!(result.messages.iter().any(|message| {
            message.role == "tool"
                && message.content["result"]["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("not valid JSON"))
        }));
    }

    #[tokio::test]
    async fn model_call_limit_returns_a_structured_result_with_usage() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([tool_response()])),
        });
        let limits = TurnLimits {
            max_model_calls: 1,
            ..TurnLimits::default()
        };
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed {
                    value: json!({"content":"hello"}),
                },
            }),
            limits,
        )
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: MODEL_CALL_LIMIT_REASON.into()
            }
        );
        assert_eq!((result.model_calls, result.tool_calls), (1, 1));
    }

    #[tokio::test]
    async fn tool_call_limit_rejects_a_whole_parallel_batch_before_execution() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([two_read_tool_response()])),
        });
        let executor = Arc::new(PausingExecutor {
            calls: AtomicUsize::new(0),
        });
        let limits = TurnLimits {
            max_tool_calls: 1,
            ..TurnLimits::default()
        };
        let result = AgentRunner::new(provider, executor.clone(), limits)
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: TOOL_CALL_LIMIT_REASON.into()
            }
        );
        assert_eq!(executor.calls.load(Ordering::SeqCst), 0);
        assert_eq!((result.model_calls, result.tool_calls), (1, 0));
    }

    #[tokio::test]
    async fn empty_model_response_is_retried_before_completing() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([
                vec![ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                }],
                vec![
                    ModelEvent::TextDelta {
                        text: "done after retry".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        )
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.assistant_text, "done after retry");
        assert!(result.messages.iter().any(|message| {
            message.role == "user"
                && message
                    .content
                    .as_str()
                    .is_some_and(|text| text.contains("[Harness retry]"))
        }));
    }

    #[tokio::test]
    async fn empty_response_that_exhausts_output_budget_retries_with_more_room() {
        struct CapturingProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            responses: Mutex<VecDeque<Vec<ModelEvent>>>,
        }
        #[async_trait]
        impl ModelProvider for CapturingProvider {
            async fn stream(
                &self,
                request: ModelRequest,
            ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
                self.requests.lock().unwrap().push(request);
                let events = self.responses.lock().unwrap().pop_front().unwrap();
                Ok(Box::pin(futures_util::stream::iter(
                    events.into_iter().map(Ok),
                )))
            }
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CapturingProvider {
            requests: requests.clone(),
            responses: Mutex::new(VecDeque::from([
                vec![
                    ModelEvent::Usage {
                        input_tokens: 10,
                        output_tokens: 100,
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("length".into()),
                    },
                ],
                vec![
                    ModelEvent::TextDelta {
                        text: "done with room".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        )
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].max_output_tokens, 100);
        assert_eq!(requests[1].max_output_tokens, 200);
    }

    #[tokio::test]
    async fn legitimate_answer_may_explain_provider_tool_markup() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([vec![
                ModelEvent::TextDelta {
                    text: "The literal <｜DSML｜tool_calls> and <tool_call> strings are provider protocol examples.".into(),
                },
                ModelEvent::Completed {
                    finish_reason: Some("stop".into()),
                },
            ]])),
        });
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        )
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 1);
        assert!(result.assistant_text.contains("<｜DSML｜tool_calls>"));
        assert!(result.assistant_text.contains("<tool_call>"));
    }

    #[tokio::test]
    async fn nonempty_truncated_response_retries_with_more_output_room() {
        struct CapturingProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            responses: Mutex<VecDeque<Vec<ModelEvent>>>,
        }
        #[async_trait]
        impl ModelProvider for CapturingProvider {
            async fn stream(
                &self,
                request: ModelRequest,
            ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
                self.requests.lock().unwrap().push(request);
                let events = self.responses.lock().unwrap().pop_front().unwrap();
                Ok(Box::pin(futures_util::stream::iter(
                    events.into_iter().map(Ok),
                )))
            }
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CapturingProvider {
            requests: requests.clone(),
            responses: Mutex::new(VecDeque::from([
                vec![
                    ModelEvent::TextDelta {
                        text: "partial answer; ".into(),
                    },
                    ModelEvent::Usage {
                        input_tokens: 10,
                        output_tokens: 100,
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("length".into()),
                    },
                ],
                vec![
                    ModelEvent::TextDelta {
                        text: "continued and verified".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let observer = Arc::new(RecordingObserver::default());
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        )
        .with_observer(observer.clone())
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(
            result.assistant_text,
            "partial answer; continued and verified"
        );
        let observed_text = observer
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(observed_text, result.assistant_text);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].max_output_tokens, 100);
        assert_eq!(requests[1].max_output_tokens, 200);
    }

    #[tokio::test]
    async fn repeated_empty_model_responses_fail_the_turn() {
        let empty = || {
            vec![ModelEvent::Completed {
                finish_reason: Some("stop".into()),
            }]
        };
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([empty(), empty(), empty()])),
        });
        let result = AgentRunner::new(
            provider,
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        )
        .run(request(), CancellationToken::new())
        .await
        .unwrap();

        assert_eq!(result.model_calls, 3);
        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: EMPTY_MODEL_RESPONSE_REASON.into()
            }
        );
    }

    #[tokio::test]
    async fn completed_event_ends_model_call_without_waiting_for_transport_eof() {
        let runner = AgentRunner::new(
            Arc::new(CompletedWithoutEofProvider),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        );

        let result = tokio::time::timeout(
            Duration::from_millis(250),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("semantic completion must not wait for connection EOF")
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "complete");
    }

    #[tokio::test]
    async fn an_idle_stream_without_events_is_retried_once() {
        let limits = TurnLimits {
            model_stream_idle_seconds: 1,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(IdleOnceProvider {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("an eventless idle stream should be retried")
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.assistant_text, "recovered");
    }

    #[tokio::test]
    async fn an_idle_stream_with_only_private_liveness_is_retried_once() {
        let limits = TurnLimits {
            model_stream_idle_seconds: 1,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(LivenessOnlyIdleOnceProvider {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("a liveness-only idle stream should be retried")
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.assistant_text, "recovered");
    }

    #[tokio::test]
    async fn a_stream_error_without_usable_output_is_retried_once() {
        let runner = AgentRunner::new(
            Arc::new(StreamErrorOnceProvider {
                calls: AtomicUsize::new(0),
            }),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            TurnLimits::default(),
        );

        let result = runner
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.assistant_text, "recovered");
    }

    #[tokio::test]
    async fn idle_model_stream_fails_and_preserves_partial_text() {
        let limits = TurnLimits {
            model_stream_idle_seconds: 1,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(IdleAfterTextProvider),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("idle model stream must fail within its deadline")
        .unwrap();

        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: MODEL_STREAM_IDLE_TIMEOUT_REASON.into(),
            }
        );
        assert_eq!(result.assistant_text, "partial");
    }

    #[tokio::test]
    async fn cancelling_a_stalled_stream_preserves_partial_text() {
        let limits = TurnLimits {
            model_stream_idle_seconds: 5,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(IdleAfterTextProvider),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_millis(250),
            runner.run(request(), cancellation),
        )
        .await
        .expect("cancellation must interrupt a stalled model stream")
        .unwrap();

        assert_eq!(result.status, AgentRunStatus::Cancelled);
        assert_eq!(result.assistant_text, "partial");
    }

    #[tokio::test]
    async fn model_start_is_covered_by_the_idle_deadline() {
        let limits = TurnLimits {
            model_stream_idle_seconds: 1,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(NeverStartsProvider),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("model start must be covered by the idle deadline")
        .unwrap();

        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: MODEL_STREAM_IDLE_TIMEOUT_REASON.into(),
            }
        );
    }

    #[tokio::test]
    async fn elapsed_turn_deadline_fires_even_when_model_stream_is_active() {
        let limits = TurnLimits {
            max_elapsed_seconds: 1,
            model_stream_idle_seconds: 5,
            ..TurnLimits::default()
        };
        let runner = AgentRunner::new(
            Arc::new(NeverIdleProvider),
            Arc::new(FakeExecutor {
                outcome: AgentToolResult::Completed { value: json!(null) },
            }),
            limits,
        );

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            runner.run(request(), CancellationToken::new()),
        )
        .await
        .expect("turn deadline must interrupt an active model stream")
        .unwrap();

        assert_eq!(
            result.status,
            AgentRunStatus::Failed {
                reason: TURN_ELAPSED_TIMEOUT_REASON.into(),
            }
        );
    }

    #[tokio::test]
    async fn non_pausing_independent_reads_execute_concurrently() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([
                two_read_tool_response(),
                vec![
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let executor = Arc::new(ConcurrentReadExecutor {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let result = AgentRunner::new(provider, executor.clone(), TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.tool_calls, 2);
        assert_eq!(executor.max_active.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_interactive_call_defers_later_calls_without_losing_protocol_results() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([two_read_tool_response()])),
        });
        let executor = Arc::new(PausingExecutor {
            calls: AtomicUsize::new(0),
        });
        let result = AgentRunner::new(provider, executor.clone(), TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert!(matches!(
            result.status,
            AgentRunStatus::AwaitingApproval { .. }
        ));
        assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
        assert!(result.messages.iter().any(|message| {
            message.role == "tool"
                && message.content["tool_call_id"] == "call_b"
                && message.content["result"]["deferred"] == true
        }));
    }

    #[tokio::test]
    async fn multiple_tool_results_merge_into_one_follow_up_step() {
        let requests = Arc::new(Mutex::new(Vec::<ModelRequest>::new()));
        struct CapturingProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            responses: Mutex<VecDeque<Vec<ModelEvent>>>,
        }
        #[async_trait]
        impl ModelProvider for CapturingProvider {
            async fn stream(
                &self,
                request: ModelRequest,
            ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
                self.requests.lock().unwrap().push(request);
                let events = self.responses.lock().unwrap().pop_front().unwrap();
                Ok(Box::pin(futures_util::stream::iter(
                    events.into_iter().map(Ok),
                )))
            }
        }
        struct EchoExecutor;
        #[async_trait]
        impl AgentToolExecutor for EchoExecutor {
            async fn execute(
                &self,
                call_id: &str,
                _: &str,
                _: Value,
                _: &CancellationToken,
            ) -> AgentToolResult {
                AgentToolResult::Completed {
                    value: json!({"call_id": call_id}),
                }
            }
        }
        let provider = Arc::new(CapturingProvider {
            requests: requests.clone(),
            responses: Mutex::new(VecDeque::from([
                vec![
                    ModelEvent::ToolCallDelta {
                        index: 0,
                        id: Some("call_a".into()),
                        name: Some("read_file".into()),
                        arguments_delta: r#"{"path":"a.txt"}"#.into(),
                        provider_metadata: None,
                    },
                    ModelEvent::ToolCallDelta {
                        index: 1,
                        id: Some("call_b".into()),
                        name: Some("read_file".into()),
                        arguments_delta: r#"{"path":"b.txt"}"#.into(),
                        provider_metadata: None,
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("tool_calls".into()),
                    },
                ],
                vec![
                    ModelEvent::TextDelta {
                        text: "both complete".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });

        let result = AgentRunner::new(provider, Arc::new(EchoExecutor), TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!((result.model_calls, result.tool_calls), (2, 2));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let tool_results = captured[1]
            .messages
            .iter()
            .filter(|message| message.role == "tool")
            .map(|message| message.content["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(tool_results, vec!["call_a", "call_b"]);
    }

    #[tokio::test]
    async fn context_change_preserves_goals_and_all_compaction_context() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([
                tool_response(),
                vec![
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::ContextChanged {
                retired_system_content: json!("Chat instructions"),
                value: json!({"mode":"work"}),
                tools: vec![],
                system_messages: vec![ModelMessage {
                    role: "system".into(),
                    content: json!("Work instructions"),
                }],
            },
        });
        let mut input = request();
        for content in [
            json!("Chat instructions"),
            json!({"persistent_goal":"Finish the complete report"}),
            json!({"context":[{"source":"internal://conversation-compaction","content":"Use the earlier blue color requirement"}]}),
            json!({"context_overflow_summary":"Keep the original acceptance conditions"}),
        ] {
            input.messages.insert(
                0,
                ModelMessage {
                    role: "system".into(),
                    content,
                },
            );
        }
        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(input, CancellationToken::new())
            .await
            .unwrap();
        let encoded = serde_json::to_string(&result.messages).unwrap();
        for retained in [
            "persistent_goal",
            "blue color requirement",
            "context_overflow_summary",
            "original acceptance conditions",
            "Work instructions",
        ] {
            assert!(encoded.contains(retained), "missing {retained}");
        }
        assert!(!encoded.contains("Chat instructions"));
    }

    #[tokio::test]
    async fn tool_search_loads_external_schema_only_after_discovery() {
        let requests = Arc::new(Mutex::new(Vec::<ModelRequest>::new()));
        struct CapturingProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            responses: Mutex<VecDeque<Vec<ModelEvent>>>,
        }
        #[async_trait]
        impl ModelProvider for CapturingProvider {
            async fn stream(
                &self,
                request: ModelRequest,
            ) -> Result<s_code_model_gateway::ModelStream, GatewayError> {
                self.requests.lock().unwrap().push(request);
                let events = self.responses.lock().unwrap().pop_front().unwrap();
                Ok(Box::pin(futures_util::stream::iter(
                    events.into_iter().map(Ok),
                )))
            }
        }
        let call = |id: &str, name: &str, arguments: &str| {
            vec![
                ModelEvent::ToolCallDelta {
                    index: 0,
                    id: Some(id.into()),
                    name: Some(name.into()),
                    arguments_delta: arguments.into(),
                    provider_metadata: None,
                },
                ModelEvent::Completed {
                    finish_reason: Some("tool_calls".into()),
                },
            ]
        };
        let provider = Arc::new(CapturingProvider {
            requests: requests.clone(),
            responses: Mutex::new(VecDeque::from([
                call("search_1", "tool_search", r#"{"query":"docs"}"#),
                call("lookup_1", "mcp.docs.lookup", r#"{"query":"reconnect"}"#),
                vec![
                    ModelEvent::TextDelta {
                        text: "done".into(),
                    },
                    ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    },
                ],
            ])),
        });
        let mut request = request();
        request.tools = vec![ToolDefinition {
            name: "tool_search".into(),
            description: "Search external tools".into(),
            parameters: json!({"type":"object"}),
        }];
        let result = AgentRunner::new(provider, Arc::new(DiscoveryExecutor), TurnLimits::default())
            .run(request, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Completed);
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0].tools.len(), 1);
        assert!(
            captured[0]
                .tools
                .iter()
                .all(|tool| tool.name != "mcp.docs.lookup")
        );
        assert!(
            captured[1]
                .tools
                .iter()
                .any(|tool| tool.name == "mcp.docs.lookup")
        );
        assert!(
            captured[2]
                .messages
                .iter()
                .any(|message| message.content["name"] == "mcp.docs.lookup")
        );
    }

    #[tokio::test]
    async fn real_http_gateway_drives_agent_tool_loop() {
        type Requests = Arc<Mutex<Vec<Value>>>;
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/chat/completions",
                post(
                    |State(requests): State<Requests>, Json(body): Json<Value>| async move {
                        let request_index = {
                            let mut requests = requests.lock().unwrap();
                            requests.push(body);
                            requests.len()
                        };
                        let frames = if request_index == 1 {
                            vec![
                                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_http","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":null}]}),
                                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                            ]
                        } else {
                            vec![
                                json!({"choices":[{"delta":{"content":"verified over HTTP"},"finish_reason":null}]}),
                                json!({"choices":[],"usage":{"prompt_tokens":18,"completion_tokens":3}}),
                                json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
                            ]
                        };
                        let mut stream = frames
                            .into_iter()
                            .map(|frame| format!("data: {frame}\n\n"))
                            .collect::<String>();
                        stream.push_str("data: [DONE]\n\n");
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(stream))
                            .unwrap()
                    },
                ),
            )
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = Arc::new(OpenAiCompatible::new(
            format!("http://{address}"),
            "integration-provider",
            Arc::new(IntegrationCredentials),
        ));
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::Completed {
                value: json!({"content":"hello from fixture"}),
            },
        });

        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "verified over HTTP");
        assert_eq!(result.model_calls, 2);
        assert_eq!(result.tool_calls, 1);
        assert_eq!((result.input_tokens, result.output_tokens), (18, 3));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let messages = captured[1]["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap();
        assert_eq!(assistant["tool_calls"][0]["id"], "call_http");
        assert!(assistant["content"].is_null());
        let tool = messages
            .iter()
            .find(|message| message["role"] == "tool")
            .unwrap();
        assert_eq!(tool["tool_call_id"], "call_http");
        assert_eq!(
            serde_json::from_str::<Value>(tool["content"].as_str().unwrap()).unwrap()["content"],
            "hello from fixture"
        );
        server.abort();
    }

    #[tokio::test]
    async fn anthropic_http_gateway_drives_agent_tool_loop() {
        type Requests = Arc<Mutex<Vec<Value>>>;
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/messages",
                post(
                    |State(requests): State<Requests>, Json(body): Json<Value>| async move {
                        let request_index = {
                            let mut requests = requests.lock().unwrap();
                            requests.push(body);
                            requests.len()
                        };
                        let frames = if request_index == 1 {
                            vec![
                                json!({"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}),
                                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"anthropic_call","name":"read_file","input":{}}}),
                                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.txt\"}"}}),
                                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
                            ]
                        } else {
                            vec![
                                json!({"type":"message_start","message":{"usage":{"input_tokens":7,"output_tokens":0}}}),
                                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"verified through Anthropic"}}),
                                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
                            ]
                        };
                        let stream = frames
                            .into_iter()
                            .map(|frame| format!("data: {frame}\n\n"))
                            .collect::<String>();
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(stream))
                            .unwrap()
                    },
                ),
            )
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = Arc::new(AnthropicMessages::new(
            format!("http://{address}"),
            "integration-provider",
            Arc::new(IntegrationCredentials),
        ));
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::Completed {
                value: json!({"content":"hello from fixture"}),
            },
        });

        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "verified through Anthropic");
        assert_eq!((result.model_calls, result.tool_calls), (2, 1));
        assert_eq!((result.input_tokens, result.output_tokens), (12, 5));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let messages = captured[1]["messages"].as_array().unwrap();
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["id"], "anthropic_call");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "anthropic_call");
        server.abort();
    }

    #[tokio::test]
    async fn gemini_http_gateway_preserves_signature_through_agent_tool_loop() {
        type Requests = Arc<Mutex<Vec<Value>>>;
        let requests: Requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/models/fake:streamGenerateContent",
                post(
                    |State(requests): State<Requests>, Json(body): Json<Value>| async move {
                        let request_index = {
                            let mut requests = requests.lock().unwrap();
                            requests.push(body);
                            requests.len()
                        };
                        let frame = if request_index == 1 {
                            json!({
                                "responseId":"gemini-response-1",
                                "candidates":[{"content":{"role":"model","parts":[{
                                    "functionCall":{"id":"gemini_call","name":"read_file","args":{"path":"a.txt"}},
                                    "thoughtSignature":"opaque-gemini-signature"
                                }]},"finishReason":"STOP"}],
                                "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":1}
                            })
                        } else {
                            json!({
                                "responseId":"gemini-response-2",
                                "candidates":[{"content":{"role":"model","parts":[{"text":"verified through Gemini"}]},"finishReason":"STOP"}],
                                "usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}
                            })
                        };
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(format!("data: {frame}\n\n")))
                            .unwrap()
                    },
                ),
            )
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = Arc::new(GeminiGenerateContent::new(
            format!("http://{address}"),
            "integration-provider",
            Arc::new(IntegrationCredentials),
        ));
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::Completed {
                value: json!({"content":"hello from fixture"}),
            },
        });

        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Completed);
        assert_eq!(result.assistant_text, "verified through Gemini");
        assert_eq!((result.model_calls, result.tool_calls), (2, 1));
        assert_eq!((result.input_tokens, result.output_tokens), (12, 4));
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let contents = captured[1]["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            "opaque-gemini-signature"
        );
        assert_eq!(contents[1]["parts"][0]["functionCall"]["id"], "gemini_call");
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["id"],
            "gemini_call"
        );
        server.abort();
    }

    #[tokio::test]
    async fn approval_pauses_without_another_model_call() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::from([tool_response()])),
        });
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::AwaitingApproval {
                detail: json!({"approval_id":"apr_1"}),
            },
        });
        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), CancellationToken::new())
            .await
            .unwrap();
        assert!(matches!(
            result.status,
            AgentRunStatus::AwaitingApproval { .. }
        ));
        assert_eq!(result.model_calls, 1);
    }

    #[tokio::test]
    async fn cancellation_stops_before_model_access() {
        let provider = Arc::new(FakeProvider {
            responses: Mutex::new(VecDeque::new()),
        });
        let executor = Arc::new(FakeExecutor {
            outcome: AgentToolResult::Failed {
                error: "unused".into(),
            },
        });
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = AgentRunner::new(provider, executor, TurnLimits::default())
            .run(request(), cancellation)
            .await
            .unwrap();
        assert_eq!(result.status, AgentRunStatus::Cancelled);
        assert_eq!(result.model_calls, 0);
    }
}
