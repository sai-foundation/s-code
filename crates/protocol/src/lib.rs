use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;
use ulid::Ulid;

pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
pub struct Id(pub String);

impl Id {
    pub fn new(prefix: &str) -> Self {
        Self(format!("{prefix}_{}", Ulid::new()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Scope {
    pub organization_id: Id,
    pub team_id: Id,
    pub actor_id: Id,
    pub goal_id: Option<Id>,
    pub task_id: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityMaturity {
    Experimental,
    Preview,
    Stable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Capability {
    pub id: String,
    pub version: String,
    pub maturity: CapabilityMaturity,
    pub enabled: bool,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CapabilityManifest {
    pub protocol_version: String,
    pub server_version: String,
    pub capabilities: Vec<Capability>,
    /// Contracts are advertised even when their capability is disabled. This lets
    /// clients and future providers integrate without treating availability as
    /// protocol support.
    #[serde(default)]
    pub contracts: Vec<CapabilityContract>,
}

pub fn protocol_major(version: &str) -> Option<u64> {
    version.split('.').next()?.parse().ok()
}

impl CapabilityManifest {
    pub fn is_protocol_compatible(&self, client_version: &str) -> bool {
        protocol_major(&self.protocol_version)
            .zip(protocol_major(client_version))
            .is_some_and(|(server, client)| server == client)
    }

    pub fn supports(&self, capability_id: &str, client_major: u64) -> bool {
        self.capabilities.iter().any(|capability| {
            capability.id == capability_id
                && capability.enabled
                && protocol_major(&capability.version)
                    .is_some_and(|version| version == client_major)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CapabilityContract {
    pub capability_id: String,
    pub version: String,
    pub provider_interface: String,
    pub input_schema: String,
    pub output_schema: String,
    pub event_types: Vec<String>,
    pub permission_scopes: Vec<String>,
    pub policy_actions: Vec<String>,
    pub lifecycle: Vec<String>,
    pub cancellation_semantics: String,
    pub recovery_semantics: String,
    pub audit_events: Vec<String>,
    pub compatibility: String,
    pub feature_flag: String,
}

/// Stable protocol contracts for capabilities intentionally unavailable in the
/// first release. Adding an implementation must not require changing these
/// boundaries or the Agent core.
pub fn deferred_capability_contracts() -> Vec<CapabilityContract> {
    vec![
        contract(
            "platform.windows_native",
            "PlatformRuntime",
            "ProcessSpec(program,args,cwd_uri,environment_handles,limits)",
            "ProcessOutput(exit_code,stdout,stderr,truncated)",
            &["platform.process.started", "platform.process.exited"],
            &["device.execute"],
            &[
                "process.execute",
                "filesystem.read",
                "filesystem.write",
                "network.connect",
            ],
            &["discover", "prepare", "execute", "cancel", "cleanup"],
            "Cancel the complete Job Object process tree; cancellation is idempotent.",
            "No process is assumed recoverable after device restart; incomplete executions become interrupted.",
            &[
                "platform.execution.requested",
                "platform.execution.completed",
            ],
            "file URI paths and structured argv only; unknown fields are ignored; v1 fields are never reinterpreted",
            "platform.windows_native",
        ),
        contract(
            "runner.microvm",
            "RunnerProvider",
            "RunnerRequest(scope,workspace_snapshot,tool_plan,network_policy,secret_handles,budget)",
            "RunnerResult(status,artifacts,evidence,usage)",
            &[
                "runner.queued",
                "runner.started",
                "runner.checkpointed",
                "runner.completed",
                "runner.failed",
            ],
            &["runner.submit", "runner.cancel", "artifact.read"],
            &[
                "runner.allocate",
                "network.egress",
                "secret.redeem",
                "artifact.publish",
            ],
            &[
                "submit",
                "lease",
                "start",
                "checkpoint",
                "complete",
                "release",
            ],
            "Cancellation revokes the lease and secret handles, then destroys the isolated workload.",
            "A replacement runner resumes only from a signed checkpoint and idempotency key.",
            &[
                "runner.requested",
                "runner.leased",
                "runner.terminated",
                "artifact.published",
            ],
            "provider-neutral snapshots and artifacts; consumers tolerate additional event and evidence fields",
            "runner.microvm",
        ),
        contract(
            "session.realtime_multiuser",
            "SessionCollaborationProvider",
            "SessionCommand(scope,participant_id,role,expected_revision,operation)",
            "SessionRevision(revision,accepted,conflicts)",
            &[
                "participant.joined",
                "participant.left",
                "session.command.accepted",
                "artifact.conflict",
            ],
            &["session.join", "session.command", "session.moderate"],
            &["session.mutate", "approval.decide", "artifact.merge"],
            &["join", "synchronize", "command", "reconcile", "leave"],
            "A participant may cancel only commands they own unless granted session.moderate.",
            "Reconnect replays the event log after the last acknowledged revision; stale commands fail preconditions.",
            &[
                "session.participant.changed",
                "session.command.recorded",
                "session.conflict.resolved",
            ],
            "append-only revisions; unknown operations/events are retained but not executed by older clients",
            "session.realtime_multiuser",
        ),
        contract(
            "task.durable",
            "DurableTaskScheduler",
            "TaskRunRequest(scope,plan,idempotency_key,budget,checkpoint_policy)",
            "TaskRunState(run_id,status,lease,checkpoint,usage,result)",
            &[
                "task.queued",
                "task.leased",
                "task.checkpointed",
                "task.paused",
                "task.completed",
                "task.failed",
            ],
            &["task.submit", "task.pause", "task.resume", "task.cancel"],
            &["task.execute", "budget.consume", "checkpoint.write"],
            &["queued", "leased", "running", "paused", "terminal"],
            "Cancel is durable, idempotent, revokes leases and prevents future tool dispatch.",
            "Expired leases may be reacquired; only signed compatible checkpoints resume and all tools require idempotency keys.",
            &[
                "task.state.changed",
                "task.lease.changed",
                "task.budget.consumed",
                "task.killed",
            ],
            "monotonic state transitions and checkpoint schema versions; incompatible checkpoints fail closed",
            "task.durable",
        ),
        contract(
            "agent.multi",
            "AgentOrchestrator",
            "AgentDagRequest(scope,parent_run,nodes,edges,budget_tree,artifact_contracts)",
            "AgentDagResult(status,node_results,artifacts,usage,conflicts)",
            &[
                "agent.child.started",
                "agent.child.completed",
                "agent.artifact.proposed",
                "agent.conflict.detected",
            ],
            &["agent.delegate", "agent.cancel_child", "artifact.merge"],
            &[
                "agent.spawn",
                "budget.delegate",
                "artifact.claim",
                "artifact.merge",
            ],
            &["plan", "dispatch", "coordinate", "verify", "terminal"],
            "Cancelling a parent recursively requests child cancellation without hiding already committed audit events.",
            "The DAG and causal trace are durable; retry requires the node idempotency key and artifact revision preconditions.",
            &[
                "agent.delegated",
                "agent.budget.delegated",
                "agent.artifact.changed",
                "agent.conflict.resolved",
            ],
            "DAG nodes, causal links and artifacts are versioned; unknown node kinds remain undispatched",
            "agent.multi",
        ),
        contract(
            "ide.jetbrains",
            "IdeClientAdapter",
            "IdeContextV1(scope,workspace_uri,document_uri,language,selection,diagnostics,revision)",
            "IdeSessionView(session,turn,events,diff,approvals,capabilities)",
            &[
                "ide.context.updated",
                "ide.session.selected",
                "ide.diff.reviewed",
                "ide.approval.decided",
            ],
            &["ide.context.write", "session.read", "approval.decide"],
            &[
                "context.submit",
                "session.select",
                "diff.review",
                "approval.decide",
            ],
            &[
                "discover",
                "connect",
                "synchronize",
                "operate",
                "disconnect",
            ],
            "Disconnect cancels only in-flight client requests; daemon Turns continue according to their own lifecycle.",
            "Reconnect uses the shared session event cursor and resubmits context only with a matching document revision.",
            &[
                "ide.connected",
                "ide.context.submitted",
                "ide.review.recorded",
            ],
            "IDE Capability Protocol v1 uses URI paths and zero-based ranges; unknown context fields and events are ignored safely",
            "ide.jetbrains",
        ),
        contract(
            "connector.enterprise",
            "EnterpriseConnectorProvider",
            "ConnectorRequest(scope,kind,operation,resource,expected_version,idempotency_key,credential_handle,payload)",
            "ConnectorResult(external_id,version,status,evidence,rate_limit)",
            &[
                "connector.requested",
                "connector.approval_required",
                "connector.completed",
                "connector.failed",
                "connector.webhook.received",
            ],
            &[
                "connector.read",
                "connector.write",
                "connector.admin",
                "webhook.receive",
            ],
            &[
                "external.read",
                "external.write",
                "secret.redeem",
                "webhook.verify",
            ],
            &[
                "discover",
                "configure",
                "authorize",
                "execute",
                "retry",
                "revoke",
            ],
            "Cancellation stops retries and revokes request-scoped credentials; an acknowledged external write remains audited.",
            "Writes retry only with the same idempotency key and expected resource version; webhook cursors resume durably.",
            &[
                "connector.configured",
                "connector.credential.redeemed",
                "connector.write.completed",
                "connector.revoked",
            ],
            "provider kinds include source-control, work, chat, CI, SIEM and secrets; additive operations are tolerated and unknown writes fail closed",
            "connector.enterprise",
        ),
        contract(
            "extension.enterprise_runtime",
            "EnterpriseExtensionRuntime",
            "ExtensionInstallRequest(scope,signed_manifest,artifact_handle,expected_digest)",
            "ExtensionInstallResult(extension_id,version,status,effective_permissions)",
            &[
                "extension.validating",
                "extension.installed",
                "extension.upgraded",
                "extension.revoked",
            ],
            &[
                "extension.read_registry",
                "extension.install",
                "extension.revoke",
            ],
            &[
                "extension.load",
                "filesystem.read",
                "network.connect",
                "secret.redeem",
                "tool.register",
            ],
            &[
                "discover", "verify", "stage", "activate", "revoke", "remove",
            ],
            "Cancellation removes staged artifacts; an active extension is disabled atomically before cleanup.",
            "Restart re-verifies signature, checksum, compatibility and revocation before loading an installed extension.",
            &[
                "extension.install.requested",
                "extension.install.completed",
                "extension.permission.used",
                "extension.revoked",
            ],
            "signed v1 manifests are immutable; upgrades and revocations require a higher sequence; unknown permissions fail closed",
            "extension.enterprise_runtime",
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn contract(
    id: &str,
    provider: &str,
    input: &str,
    output: &str,
    events: &[&str],
    permissions: &[&str],
    actions: &[&str],
    lifecycle: &[&str],
    cancellation: &str,
    recovery: &str,
    audit: &[&str],
    compatibility: &str,
    feature_flag: &str,
) -> CapabilityContract {
    CapabilityContract {
        capability_id: id.into(),
        version: "1".into(),
        provider_interface: provider.into(),
        input_schema: input.into(),
        output_schema: output.into(),
        event_types: events.iter().map(|value| (*value).into()).collect(),
        permission_scopes: permissions.iter().map(|value| (*value).into()).collect(),
        policy_actions: actions.iter().map(|value| (*value).into()).collect(),
        lifecycle: lifecycle.iter().map(|value| (*value).into()).collect(),
        cancellation_semantics: cancellation.into(),
        recovery_semantics: recovery.into(),
        audit_events: audit.iter().map(|value| (*value).into()).collect(),
        compatibility: compatibility.into(),
        feature_flag: feature_flag.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub id: Id,
    pub scope: Scope,
    pub session_id: Id,
    pub turn_id: Id,
    pub tool: String,
    pub arguments: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyResult {
    pub decision: PolicyDecision,
    pub policy_id: String,
    pub policy_version: String,
    pub reason: String,
    pub requires_approval: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: Id,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub scope: Scope,
    pub session_id: Option<Id>,
    pub turn_id: Option<Id>,
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: Value,
}

/// A client-facing event envelope. Internal audit events intentionally retain
/// their original representation; this projection adds the stable ownership
/// and lifecycle fields required by interactive clients.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ClientEvent {
    pub id: Id,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub session_id: Option<Id>,
    pub turn_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Id>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub payload_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<ClientNotification>,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientNotification {
    MessageCreated {
        item_id: Id,
    },
    AgentMessageDelta {
        item_id: Id,
        delta: String,
        /// UTF-8 byte offset at which `delta` must be appended. Older event
        /// journals may omit it; live producers always set it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        byte_offset: Option<u64>,
    },
    TurnStatusChanged {
        status: TurnStatus,
        error_code: Option<String>,
    },
    ToolCallChanged {
        item_id: Id,
        model_call_id: Option<Id>,
        tool: String,
        display: String,
        status: TranscriptItemStatus,
    },
    ApprovalRequested {
        request_id: Id,
        item_id: Id,
        tool_item_id: Id,
        model_call_id: Option<Id>,
        tool: String,
        summary: String,
    },
    QuestionRequested {
        request_id: Id,
        item_id: Id,
        questions: Vec<QuestionPrompt>,
    },
    ArtifactCreated {
        item_id: Id,
        artifact_id: Id,
        title: String,
        media_type: String,
    },
    TurnInputChanged {
        input_id: Id,
        target_turn_id: Id,
        resulting_turn_id: Option<Id>,
        mode: TurnInputMode,
        status: TurnInputStatus,
    },
    PlanUpdated {
        item_id: Id,
        title: Option<String>,
        steps: Vec<TranscriptPlanStep>,
    },
    ContextCompacted {
        item_id: Id,
        omitted_messages: u32,
        truncated_messages: u32,
        estimated_tokens: u64,
    },
    ModelRerouted {
        from_model: Option<String>,
        to_model: String,
        reason: String,
    },
    ReasoningSummaryDelta {
        item_id: Id,
        delta: String,
    },
    McpProgressChanged {
        item_id: Id,
        server: String,
        tool: String,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    },
    UsageRecorded {
        item_id: Id,
        model: String,
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        model_calls: u32,
        tool_calls: u32,
    },
    DurableTaskChanged {
        task_id: Id,
        status: DurableTaskStatus,
        attempt: u32,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: u64,
    },
    AgentRunChanged {
        item_id: Id,
        agent_id: Id,
        parent_agent_id: Option<Id>,
        status: DurableTaskStatus,
        label: String,
        attempt: u32,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: u64,
    },
    HookChanged {
        item_id: Id,
        event: String,
        handler: String,
        status: TranscriptItemStatus,
        input_modified: bool,
    },
    BackgroundTerminalChanged {
        terminal_id: Id,
        status: BackgroundTerminalStatus,
        output_byte_length: u64,
        output_truncated: bool,
        exit_code: Option<i32>,
        artifact_id: Option<Id>,
        revision: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Idle,
    PreparingContext,
    CallingModel,
    AwaitingInput,
    AwaitingApproval,
    RunningTool,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub protocol_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_instance_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DaemonSettings {
    pub organization_id: Id,
    pub team_id: Id,
    pub actor_id: Id,
    pub workspace_uri: String,
    pub default_model: String,
    pub default_title: String,
    pub telemetry_enabled: bool,
    pub max_context_tokens: u32,
}

impl Default for DaemonSettings {
    fn default() -> Self {
        Self {
            organization_id: Id("org_local".into()),
            team_id: Id("team_local".into()),
            actor_id: Id("user_local".into()),
            workspace_uri: "file:///workspace".into(),
            default_model: "deepseek/deepseek-v4-flash".into(),
            default_title: "Team coding session".into(),
            telemetry_enabled: false,
            max_context_tokens: 32_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Archived,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Session {
    pub id: Id,
    pub scope: Scope,
    pub workspace_uri: String,
    pub title: String,
    pub model: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Manual,
    AcceptEdits,
    Plan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionPreferences {
    pub session_id: Id,
    pub permission_mode: PermissionMode,
    pub assistant_alias: String,
    pub source: String,
    pub locked_reason: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionPreferences {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ModelCatalogEntry {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub context_window: Option<u64>,
    pub supports_reasoning: Option<bool>,
    pub cost_tier: Option<String>,
    pub available: bool,
    pub recommended: bool,
    pub source: String,
    pub locked_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PermissionProfile {
    pub mode: PermissionMode,
    pub label: String,
    pub description: String,
    pub file_changes: String,
    pub commands: String,
    pub network: String,
    pub source: String,
    pub locked_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct CreateSession {
    pub scope: Scope,
    pub workspace_uri: String,
    pub title: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionLifecycleRequest {
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateSession {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ForkSession {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RetryTurn {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct RetryTurnResult {
    pub source_turn_id: Id,
    pub session: Session,
    pub turn: Turn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionForkMetadata {
    pub session_id: Id,
    pub parent_session_id: Id,
    pub source_turn_id: Option<Id>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SideConversationStatus {
    Active,
    Promoted,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SideConversation {
    pub id: Id,
    pub scope: Scope,
    pub source_session_id: Id,
    pub session_id: Id,
    pub source_turn_id: Option<Id>,
    pub status: SideConversationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateSideConversation {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<Id>,
    pub prompt: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SideConversationStart {
    pub conversation: SideConversation,
    pub session: Session,
    pub turn: Turn,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionBranchNode {
    pub session: Session,
    pub parent_session_id: Option<Id>,
    pub source_turn_id: Option<Id>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionBranchTree {
    pub root_session_id: Id,
    pub nodes: Vec<SessionBranchNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Message {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub role: String,
    pub content: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptItemKind {
    UserMessage,
    AgentMessage,
    ReasoningSummary,
    Plan,
    ToolCall,
    CommandExecution,
    FileRead,
    FileSearch,
    FileChange,
    McpCall,
    Hook,
    DynamicTool,
    Question,
    Approval,
    Diff,
    Artifact,
    Warning,
    ContextCompaction,
    ModelReroute,
    Usage,
    AgentStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptItemStatus {
    Pending,
    Started,
    Streaming,
    AwaitingInput,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
    Denied,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TranscriptPlanStep {
    pub text: String,
    pub status: TranscriptPlanStepStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct McpProgress {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ApprovalRequest {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub item_id: Id,
    pub tool: String,
    pub summary: String,
    pub target: Option<String>,
    pub impact_scope: String,
    pub policy_reason: String,
    pub risk: ApprovalRisk,
    pub allowed_scopes: Vec<ApprovalScope>,
    pub requested_by: Id,
    pub decision_actors: Vec<Id>,
    pub requested_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: ApprovalStatus,
    pub approval_steps_completed: u32,
    pub approval_steps_required: u32,
    pub audit_event_id: Option<Id>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuestionPrompt {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuestionAnswer {
    pub question_id: String,
    pub answer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Expired,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct QuestionRequest {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub item_id: Id,
    pub questions: Vec<QuestionPrompt>,
    pub allow_other: bool,
    pub requested_by: Id,
    pub requested_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub status: QuestionStatus,
    pub answers: Vec<QuestionAnswer>,
    pub answered_by: Option<Id>,
    pub answered_at: Option<DateTime<Utc>>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AttachmentMetadata {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Option<Id>,
    pub file_name: String,
    pub media_type: String,
    pub byte_length: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Attachment {
    pub metadata: AttachmentMetadata,
    pub content_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateAttachment {
    pub scope: Scope,
    pub file_name: String,
    pub media_type: String,
    pub content_base64: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptItemContent {
    Message {
        role: String,
        content: Value,
        attachments: Vec<AttachmentMetadata>,
    },
    ToolCall {
        tool_call_id: Id,
        tool: String,
        display: String,
        policy_reason: String,
        result_summary: Option<String>,
    },
    McpCall {
        tool_call_id: Id,
        server: String,
        tool: String,
        namespaced_tool: String,
        display: String,
        policy_reason: String,
        result_summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<McpProgress>,
    },
    Hook {
        event: String,
        handler: String,
        source_uri: String,
        trust: ExtensionTrust,
        input_modified: bool,
        result_summary: Option<String>,
    },
    Approval {
        request: Box<ApprovalRequest>,
    },
    ReasoningSummary {
        text: String,
    },
    Plan {
        title: Option<String>,
        steps: Vec<TranscriptPlanStep>,
    },
    Question {
        request: Box<QuestionRequest>,
    },
    Warning {
        code: String,
        message: String,
    },
    ContextCompaction {
        omitted_messages: u32,
        truncated_messages: u32,
        estimated_tokens: u64,
    },
    ModelReroute {
        from_model: Option<String>,
        to_model: String,
        reason: String,
    },
    Usage {
        model: String,
        input_tokens: u64,
        output_tokens: u64,
        total_tokens: u64,
        model_calls: u32,
        tool_calls: u32,
    },
    AgentStatus {
        agent_id: Id,
        parent_agent_id: Option<Id>,
        label: String,
    },
    Artifact {
        artifact_id: Id,
        media_type: String,
        title: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TranscriptDetailReference {
    pub href: String,
    pub media_type: String,
    pub byte_length: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TranscriptItem {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub kind: TranscriptItemKind,
    pub status: TranscriptItemStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub summary: String,
    pub content: TranscriptItemContent,
    pub detail: Option<TranscriptDetailReference>,
    pub approval_id: Option<Id>,
    pub policy_id: Option<String>,
    pub audit_event_id: Option<Id>,
    pub truncated: bool,
    pub retryable: bool,
    pub cancellable: bool,
    pub capability_version: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ArtifactMetadata {
    pub id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub item_id: Id,
    pub title: String,
    pub media_type: String,
    pub byte_length: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Artifact {
    pub metadata: ArtifactMetadata,
    pub content: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    ReviewReport,
    TestReport,
    Diff,
    Document,
    Image,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ArtifactIndexEntry {
    pub metadata: ArtifactMetadata,
    pub kind: ArtifactKind,
    pub session_title: String,
    pub workspace_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ArtifactPage {
    pub artifacts: Vec<ArtifactIndexEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    McpServer,
    Skill,
    Hook,
    Plugin,
    App,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionStatus {
    Available,
    Installed,
    Connected,
    Disabled,
    NeedsAuthentication,
    Locked,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionTrust {
    BuiltIn,
    LocalConfiguration,
    SignedEnterprise,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPermissionKind {
    Tool,
    File,
    Network,
    Secret,
    Command,
    Data,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExtensionPermission {
    pub kind: ExtensionPermissionKind,
    pub value: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExtensionDescriptor {
    pub id: String,
    pub name: String,
    pub kind: ExtensionKind,
    pub description: String,
    pub version: Option<String>,
    pub publisher: Option<String>,
    pub source_uri: String,
    pub status: ExtensionStatus,
    pub trust: ExtensionTrust,
    pub permissions: Vec<ExtensionPermission>,
    pub tool_names: Vec<String>,
    pub oauth_supported: bool,
    pub authenticated: bool,
    pub signature_verified: bool,
    pub allowlisted: bool,
    pub permissions_sha256: Option<String>,
    pub locked_reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpServerSpec {
    pub id: String,
    pub program: String,
    pub args: Vec<String>,
    pub environment_handles: BTreeMap<String, String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpHttpServerSpec {
    pub id: String,
    pub endpoint: String,
    pub header_handles: BTreeMap<String, String>,
    #[serde(default)]
    pub oauth: Option<McpOAuthSpec>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpOAuthSpec {
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpOAuthStatus {
    pub server_id: String,
    pub supported: bool,
    pub authenticated: bool,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpOAuthLaunch {
    pub server_id: String,
    pub authorization_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpOAuthDiscovery {
    pub server_id: String,
    pub authorization_server: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes: Vec<String>,
    pub permissions_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PreviewMcpOAuth {
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StartMcpOAuth {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct LogoutMcpOAuth {
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExtensionInstallPreview {
    pub descriptor: ExtensionDescriptor,
    pub permissions_sha256: String,
    pub requires_restart: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExtensionConfirmation {
    pub confirmed: bool,
    pub permissions_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PreviewMcpServerInstall {
    pub scope: Scope,
    pub server: McpServerSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct InstallMcpServer {
    pub scope: Scope,
    pub server: McpServerSpec,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RemoveMcpServer {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PreviewMcpHttpServerInstall {
    pub scope: Scope,
    pub server: McpHttpServerSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct InstallMcpHttpServer {
    pub scope: Scope,
    pub server: McpHttpServerSpec,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RemoveMcpHttpServer {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpInstallation {
    pub scope: Scope,
    pub server: McpServerSpec,
    pub permissions_sha256: String,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpHttpInstallation {
    pub scope: Scope,
    pub server: McpHttpServerSpec,
    pub permissions_sha256: String,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResource {
    pub server_id: String,
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResourcePage {
    pub resources: Vec<McpResource>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResourceTemplate {
    pub server_id: String,
    pub uri_template: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResourceTemplatePage {
    pub resource_templates: Vec<McpResourceTemplate>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResourceContent {
    pub server_id: String,
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob_base64: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct McpResourceRead {
    pub contents: Vec<McpResourceContent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SkillSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source_uri: String,
    pub activation_terms: Vec<String>,
    pub mcp_dependencies: Vec<String>,
    pub auto_match: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SkillInstallation {
    pub scope: Scope,
    pub skill: SkillSpec,
    pub content_sha256: String,
    pub permissions_sha256: String,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PreviewSkillInstall {
    pub scope: Scope,
    pub skill: SkillSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct InstallSkill {
    pub scope: Scope,
    pub skill: SkillSpec,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SetSkillEnabled {
    pub scope: Scope,
    pub enabled: bool,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RemoveSkill {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HookSpec {
    pub id: String,
    pub name: String,
    pub event: HookEvent,
    pub program: String,
    pub args: Vec<String>,
    pub environment_handles: BTreeMap<String, String>,
    pub timeout_ms: u64,
    pub can_modify_input: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HookInstallation {
    pub scope: Scope,
    pub hook: HookSpec,
    pub permissions_sha256: String,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PreviewHookInstall {
    pub scope: Scope,
    pub hook: HookSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct InstallHook {
    pub scope: Scope,
    pub hook: HookSpec,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RemoveHook {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MarketplaceSourceKind {
    Local,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MarketplaceSource {
    pub name: String,
    pub source_uri: String,
    pub source_kind: MarketplaceSourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MarketplaceInstallation {
    pub scope: Scope,
    pub source: MarketplaceSource,
    pub manifest_sha256: String,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginInterface {
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default, alias = "shortDescription")]
    pub short_description: Option<String>,
    #[serde(default, alias = "longDescription")]
    pub long_description: Option<String>,
    #[serde(default, alias = "developerName")]
    pub developer_name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, alias = "websiteURL", alias = "websiteUrl")]
    pub website_url: Option<String>,
    #[serde(default, alias = "privacyPolicyURL", alias = "privacyPolicyUrl")]
    pub privacy_policy_url: Option<String>,
    #[serde(default, alias = "termsOfServiceURL", alias = "termsOfServiceUrl")]
    pub terms_of_service_url: Option<String>,
    #[serde(default, alias = "defaultPrompt")]
    pub default_prompt: Vec<String>,
    #[serde(default, alias = "brandColor")]
    pub brand_color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginAppSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, alias = "homepageURL", alias = "homepageUrl")]
    pub homepage_url: Option<String>,
    #[serde(default, alias = "mcpServerIds")]
    pub mcp_server_ids: Vec<String>,
    #[serde(default, alias = "toolNames")]
    pub tool_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginAgentSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source_uri: String,
    pub content_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginSkillAsset {
    pub skill: SkillSpec,
    pub instructions: String,
    pub content_sha256: String,
    pub permissions_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginBundle {
    pub id: String,
    pub marketplace_name: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: Option<String>,
    pub keywords: Vec<String>,
    pub source_uri: String,
    pub manifest_sha256: String,
    pub permissions_sha256: String,
    pub interface: Option<PluginInterface>,
    pub skills: Vec<PluginSkillAsset>,
    pub hooks: Vec<HookSpec>,
    pub mcp_servers: Vec<McpServerSpec>,
    pub mcp_http_servers: Vec<McpHttpServerSpec>,
    pub apps: Vec<PluginAppSpec>,
    pub agents: Vec<PluginAgentSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginInstallation {
    pub scope: Scope,
    pub bundle: PluginBundle,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginComponentSummary {
    pub kind: ExtensionKind,
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginSummary {
    pub descriptor: ExtensionDescriptor,
    pub marketplace_name: String,
    pub installed_version: Option<String>,
    pub installed_revision: Option<u64>,
    pub available_version: String,
    pub update_available: bool,
    pub components: Vec<PluginComponentSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginDetail {
    pub summary: PluginSummary,
    pub keywords: Vec<String>,
    pub interface: Option<PluginInterface>,
    pub apps: Vec<PluginAppSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PluginAppDescriptor {
    pub plugin_id: String,
    pub marketplace_name: String,
    pub app: PluginAppSpec,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PreviewMarketplaceAdd {
    pub scope: Scope,
    pub source: MarketplaceSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AddMarketplace {
    pub scope: Scope,
    pub source: MarketplaceSource,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpgradeMarketplace {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RemoveMarketplace {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct PreviewPluginInstall {
    pub scope: Scope,
    pub marketplace_name: String,
    pub plugin_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct InstallPlugin {
    pub scope: Scope,
    pub marketplace_name: String,
    pub plugin_name: String,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RemovePlugin {
    pub scope: Scope,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SetPluginEnabled {
    pub scope: Scope,
    pub enabled: bool,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTerminalStatus {
    Starting,
    Running,
    Exited,
    Failed,
    Stopped,
    Orphaned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BackgroundTerminalSpec {
    pub session_id: Id,
    pub program: String,
    pub args: Vec<String>,
    pub environment_handles: BTreeMap<String, String>,
    pub working_directory_uri: String,
    pub rows: u16,
    pub cols: u16,
    pub max_runtime_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PreviewBackgroundTerminal {
    pub scope: Scope,
    pub terminal: BackgroundTerminalSpec,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StartBackgroundTerminal {
    pub scope: Scope,
    pub terminal: BackgroundTerminalSpec,
    pub confirmation: ExtensionConfirmation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BackgroundTerminalPreview {
    pub permissions: Vec<ExtensionPermission>,
    pub permissions_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BackgroundTerminalSummary {
    pub id: Id,
    pub scope: Scope,
    pub session_id: Id,
    pub turn_id: Id,
    pub program: String,
    pub argument_count: u32,
    pub working_directory_uri: String,
    pub status: BackgroundTerminalStatus,
    pub rows: u16,
    pub cols: u16,
    pub max_runtime_seconds: u64,
    pub output_byte_length: u64,
    pub output_truncated: bool,
    pub exit_code: Option<i32>,
    pub artifact_id: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BackgroundTerminalOutput {
    pub terminal_id: Id,
    pub offset: u64,
    pub next_offset: u64,
    pub content_base64: String,
    pub eof: bool,
    pub output_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct WriteBackgroundTerminal {
    pub scope: Scope,
    pub content_base64: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ResizeBackgroundTerminal {
    pub scope: Scope,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct StopBackgroundTerminal {
    pub scope: Scope,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TranscriptSnapshot {
    pub protocol_version: String,
    /// Monotonic Team event revision captured before projection reads. A
    /// client must not replace newer local state with an older snapshot.
    pub snapshot_revision: u64,
    pub session: Session,
    pub turns: Vec<Turn>,
    pub items: Vec<TranscriptItem>,
    /// Total number of projected Items before page filtering.
    pub item_count: u64,
    /// Opaque cursor for the next older page.
    pub next_cursor: Option<String>,
    pub pending_requests: Vec<ApprovalRequest>,
    pub pending_questions: Vec<QuestionRequest>,
    pub pending_inputs: Vec<TurnInput>,
    pub attachments: Vec<AttachmentMetadata>,
    pub artifacts: Vec<ArtifactMetadata>,
    /// Exact aggregate reconstructed from all persisted Turn usage records,
    /// independent of Transcript pagination.
    #[serde(default)]
    pub usage: SessionUsage,
    pub cursor: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    pub turns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionExportFormat {
    Markdown,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionExport {
    pub session_id: Id,
    pub format: SessionExportFormat,
    pub file_name: String,
    pub media_type: String,
    pub content: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionImpactPreview {
    pub session_id: Id,
    pub title: String,
    pub status: SessionStatus,
    pub turn_count: u32,
    pub active_turn_count: u32,
    pub message_count: u32,
    pub attachment_count: u32,
    pub artifact_count: u32,
    pub branch_count: u32,
    pub stops_active_turns: bool,
    pub hides_team_session: bool,
    pub audit_evidence_preserved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum UndoPathAction {
    Restore,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UndoPathImpact {
    pub path: String,
    pub action: UndoPathAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TurnUndoImpactPreview {
    pub turn_id: Id,
    pub session_id: Id,
    pub status: TurnStatus,
    pub paths: Vec<UndoPathImpact>,
    pub conversation_messages: u32,
    pub plan_items: u32,
    pub queued_inputs: u32,
    pub session_goal_changes: u32,
    pub latest_conversation_turn: bool,
    pub protects_later_human_edits: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Project,
    Team,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryItem {
    pub id: Id,
    pub session_id: Id,
    pub memory_scope: MemoryScope,
    pub content: String,
    pub citation: String,
    pub source_uri: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateMemory {
    pub scope: Scope,
    pub memory_scope: MemoryScope,
    pub content: String,
    pub citation: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CompactSession {
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CompactSessionResult {
    pub session_id: Id,
    pub turn_id: Id,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub omitted_messages: u32,
    pub truncated_messages: u32,
    pub focus_applied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFindingStatus {
    Open,
    Fixed,
    Ignored,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ReviewLocation {
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ReviewFinding {
    pub id: String,
    pub severity: ReviewSeverity,
    pub title: String,
    pub description: String,
    pub evidence: String,
    pub suggested_fix: Option<String>,
    pub location: ReviewLocation,
    pub status: ReviewFindingStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ReviewReport {
    pub target: String,
    pub findings: Vec<ReviewFinding>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ContextSummaryItem {
    pub id: String,
    pub kind: String,
    pub source_uri: String,
    pub trust_level: String,
    pub estimated_tokens: u64,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ContextSummary {
    pub session_id: Id,
    pub max_input_tokens: u64,
    pub reserved_output_tokens: u64,
    pub conversation_tokens: u64,
    pub item_tokens: u64,
    pub total_estimated_tokens: u64,
    pub items: Vec<ContextSummaryItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct CreateMessage {
    pub scope: Scope,
    pub content: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Turn {
    pub id: Id,
    pub session_id: Id,
    pub scope: Scope,
    pub status: TurnStatus,
    pub checkpoint: Option<Value>,
    pub error_code: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateTurn {
    pub scope: Scope,
    pub content: Value,
    #[serde(default)]
    pub attachment_ids: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputMode {
    Steer,
    Queue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnInputStatus {
    Pending,
    Processing,
    Consumed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TurnInput {
    pub id: Id,
    pub session_id: Id,
    pub target_turn_id: Id,
    pub resulting_turn_id: Option<Id>,
    pub scope: Scope,
    pub mode: TurnInputMode,
    pub content: Value,
    pub status: TurnInputStatus,
    pub idempotency_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateTurnInput {
    pub scope: Scope,
    pub target_turn_id: Id,
    pub mode: TurnInputMode,
    pub content: Value,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CancelTurnInput {
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateReview {
    pub scope: Scope,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DurableTaskStatus {
    Queued,
    Leased,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct CreateDurableTask {
    pub scope: Scope,
    pub kind: String,
    pub payload: Value,
    pub idempotency_key: String,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
    #[serde(default)]
    pub max_runner_cost_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct DurableTask {
    pub id: Id,
    pub scope: Scope,
    pub kind: String,
    pub payload: Value,
    pub idempotency_key: String,
    pub status: DurableTaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
    pub consumed_cost_micros: u64,
    pub max_runner_cost_micros: u64,
    pub consumed_runner_cost_micros: u64,
    pub lease_owner: Option<String>,
    pub lease_token: Option<String>,
    pub lease_expires_at: Option<DateTime<Utc>>,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Content-free projection for task lists and dashboards.
///
/// Durable task payloads, checkpoints, results, errors and lease credentials may
/// contain prompts, command output or worker secrets. Clients that only render
/// status must use this projection instead of receiving the execution record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DurableTaskSummary {
    pub id: Id,
    pub scope: Scope,
    pub kind: String,
    pub session_id: Option<Id>,
    pub status: DurableTaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
    pub consumed_cost_micros: u64,
    pub max_runner_cost_micros: u64,
    pub consumed_runner_cost_micros: u64,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentRunSummary {
    pub id: Id,
    pub parent_id: Option<Id>,
    pub session_id: Option<Id>,
    pub goal_id: Option<Id>,
    pub team_task_id: Option<Id>,
    pub status: DurableTaskStatus,
    pub model: Option<String>,
    pub attempt: u32,
    pub consumed_cost_micros: u64,
    pub consumed_runner_cost_micros: u64,
    pub cancel_requested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Bounded, explicitly requested final Agent output.
///
/// Agent list/tree endpoints remain content-free. Clients fetch this projection
/// only when the user asks to aggregate completed Agent results.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AgentResultSummary {
    pub agent_id: Id,
    pub parent_id: Option<Id>,
    pub session_id: Id,
    pub status: DurableTaskStatus,
    pub final_message_id: Option<Id>,
    pub summary: Option<String>,
    pub summary_byte_length: u64,
    pub truncated: bool,
    pub artifact_count: u32,
    pub consumed_cost_micros: u64,
    pub consumed_runner_cost_micros: u64,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AgentWait {
    pub scope: Scope,
    /// Maximum server-side wait. Zero performs an immediate status read.
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct AgentFollowUp {
    pub scope: Scope,
    pub content: Value,
    pub idempotency_key: String,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurableTaskCheckpoint {
    pub lease_token: String,
    pub checkpoint: Value,
    pub consumed_cost_micros: u64,
    #[serde(default)]
    pub consumed_runner_cost_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurableTaskCompletion {
    pub lease_token: String,
    pub result: Value,
    pub consumed_cost_micros: u64,
    #[serde(default)]
    pub consumed_runner_cost_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DurableTaskFailure {
    pub lease_token: String,
    pub error: String,
    pub retryable: bool,
    pub consumed_cost_micros: u64,
    #[serde(default)]
    pub consumed_runner_cost_micros: u64,
}

pub const LINUX_RUNNER_TASK_KIND: &str = "runner.process.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// A runner-local, read-only or disposable `file://` mount prepared by the
    /// snapshot broker. Host paths are never inferred from repository input.
    pub root_uri: String,
    pub digest_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerProcessStep {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxRunnerTaskPayload {
    pub schema_version: u32,
    pub workspace_snapshot: WorkspaceSnapshot,
    pub tool_plan: Vec<RunnerProcessStep>,
    /// Network remains denied unless both the task and runner policy permit it.
    #[serde(default)]
    pub network_enabled: bool,
    #[serde(default)]
    pub secret_handles: Vec<String>,
    #[serde(default = "default_runner_output_limit")]
    pub output_limit_bytes: usize,
}

fn default_runner_output_limit() -> usize {
    1024 * 1024
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerStepEvidence {
    pub step: usize,
    pub program: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub duration_millis: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinuxRunnerResult {
    pub schema_version: u32,
    pub snapshot_digest_sha256: String,
    pub steps: Vec<RunnerStepEvidence>,
}

pub const MICROVM_RUNNER_TASK_KIND: &str = "runner.microvm.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicroVmArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmImage {
    /// Runner-local immutable image supplied by the release artifact broker.
    pub kernel_uri: String,
    pub kernel_sha256: String,
    pub rootfs_uri: String,
    pub rootfs_sha256: String,
    pub architecture: MicroVmArchitecture,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmResources {
    pub vcpu_count: u16,
    pub memory_mib: u32,
    pub max_runtime_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicroVmNetworkMode {
    Deny,
    AllowList,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmNetworkPolicy {
    pub mode: MicroVmNetworkMode,
    #[serde(default)]
    pub allowed_cidrs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmRunnerTaskPayload {
    pub schema_version: u32,
    pub region: String,
    pub workspace_snapshot: WorkspaceSnapshot,
    pub image: MicroVmImage,
    pub resources: MicroVmResources,
    pub network: MicroVmNetworkPolicy,
    pub tool_plan: Vec<RunnerProcessStep>,
    #[serde(default)]
    pub secret_handles: Vec<String>,
    #[serde(default = "default_runner_output_limit")]
    pub output_limit_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmIsolationEvidence {
    pub provider_id: String,
    pub instance_id: String,
    pub architecture: MicroVmArchitecture,
    pub kernel_sha256: String,
    pub rootfs_sha256: String,
    pub snapshot_sha256: String,
    pub network_mode: MicroVmNetworkMode,
    pub host_filesystem_not_mounted: bool,
    pub guest_agent_authenticated: bool,
    pub workload_destroyed: bool,
    pub attestation_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MicroVmRunnerResult {
    pub schema_version: u32,
    pub isolation: MicroVmIsolationEvidence,
    pub steps: Vec<RunnerStepEvidence>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct CancelTurn {
    pub scope: Scope,
}

pub const IDE_PROTOCOL_VERSION: &str = "1.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorRange {
    pub start: EditorPosition,
    pub end: EditorPosition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorSelection {
    pub range: EditorRange,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorDiagnostic {
    pub range: EditorRange,
    pub severity: String,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateEditorContext {
    pub scope: Scope,
    pub protocol_version: String,
    pub client_instance_id: Id,
    pub workspace_uri: String,
    pub active_document: Option<EditorDocument>,
    pub selection: Option<EditorSelection>,
    pub diagnostics: Vec<EditorDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorContext {
    pub session_id: Id,
    pub scope: Scope,
    pub protocol_version: String,
    pub client_instance_id: Id,
    pub workspace_uri: String,
    pub active_document: Option<EditorDocument>,
    pub selection: Option<EditorSelection>,
    pub diagnostics: Vec<EditorDiagnostic>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionGoalStatus {
    Active,
    Paused,
    Completed,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionGoal {
    pub id: Id,
    pub session_id: Id,
    pub scope: Scope,
    pub objective: String,
    pub status: SessionGoalStatus,
    pub auto_continue: bool,
    pub token_budget: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub continuation_count: u32,
    pub last_turn_id: Option<Id>,
    pub blocked_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct SetSessionGoal {
    pub scope: Scope,
    pub objective: String,
    #[serde(default = "default_true")]
    pub auto_continue: bool,
    pub token_budget: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionGoal {
    pub scope: Scope,
    pub objective: Option<String>,
    pub status: Option<SessionGoalStatus>,
    pub auto_continue: Option<bool>,
    pub expected_revision: u64,
    pub blocked_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ClearSessionGoal {
    pub scope: Scope,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Planned,
    Active,
    Achieved,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamGoal {
    pub id: Id,
    pub scope: Scope,
    pub title: String,
    pub outcome_definition: String,
    pub status: GoalStatus,
    pub target_date: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct ContinueTeamGoal {
    pub scope: Scope,
    pub workspace_uri: String,
    pub model: String,
    pub idempotency_key: String,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TeamGoalRunStatus {
    Active,
    Paused,
    Cancelled,
    AwaitingVerification,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamGoalRun {
    pub id: Id,
    pub scope: Scope,
    pub goal_id: Id,
    pub status: TeamGoalRunStatus,
    pub workspace_uri: String,
    pub model: String,
    pub max_attempts: u32,
    pub max_runtime_seconds: u64,
    pub max_cost_micros: u64,
    pub current_task_id: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateTeamGoalRun {
    pub scope: Scope,
    pub status: TeamGoalRunStatus,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamGoalContinuation {
    pub run: TeamGoalRun,
    pub goal: TeamGoal,
    pub task: TeamTask,
    pub session: Session,
    pub agent_task: DurableTaskSummary,
    pub auto_continue: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamGoal {
    pub scope: Scope,
    pub title: String,
    pub outcome_definition: String,
    pub target_date: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateTeamGoal {
    pub scope: Scope,
    pub title: String,
    pub outcome_definition: String,
    pub status: GoalStatus,
    pub target_date: Option<DateTime<Utc>>,
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TeamTaskStatus {
    Ready,
    InProgress,
    Blocked,
    Review,
    Verified,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamTask {
    pub id: Id,
    pub scope: Scope,
    pub goal_id: Option<Id>,
    pub source: String,
    pub title: String,
    pub priority: i32,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<Id>,
    pub status: TeamTaskStatus,
    pub acceptance_criteria: Vec<String>,
    pub required_evidence: Vec<String>,
    pub blockers: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamTask {
    pub scope: Scope,
    pub goal_id: Option<Id>,
    pub source: String,
    pub title: String,
    pub priority: i32,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<Id>,
    pub acceptance_criteria: Vec<String>,
    pub required_evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateTeamTask {
    pub scope: Scope,
    pub status: TeamTaskStatus,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<Id>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamWorkSyncResult {
    pub sequence: u64,
    pub goals: u64,
    pub tasks: u64,
    pub cancelled_goals: u64,
    pub cancelled_tasks: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamKnowledgeItem {
    pub id: Id,
    pub scope: Scope,
    pub source_uri: String,
    pub title: String,
    pub content: String,
    pub version: String,
    pub trust_level: String,
    pub permission: String,
    pub valid_until: Option<DateTime<Utc>>,
    pub owner_team_id: Id,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamKnowledgeItem {
    pub scope: Scope,
    pub source_uri: String,
    pub title: String,
    pub content: String,
    pub version: String,
    pub trust_level: String,
    pub permission: String,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct OutcomeEvidence {
    pub kind: String,
    pub uri: String,
    pub result: String,
    pub collected_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamOutcome {
    pub id: Id,
    pub scope: Scope,
    pub goal_id: Id,
    pub task_id: Id,
    pub status: String,
    pub evidence: Vec<OutcomeEvidence>,
    pub pull_request_url: Option<String>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamOutcome {
    pub scope: Scope,
    pub goal_id: Id,
    pub task_id: Id,
    pub evidence: Vec<OutcomeEvidence>,
    pub pull_request_url: Option<String>,
}

/// Content-free Team audit projection for ordinary product surfaces. The
/// original encrypted event payload remains available only to the governed
/// audit/export path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AuditEventSummary {
    pub id: Id,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub actor_id: Id,
    pub goal_id: Option<Id>,
    pub task_id: Option<Id>,
    pub session_id: Option<Id>,
    pub turn_id: Option<Id>,
    pub item_id: Option<Id>,
    pub request_id: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AuditEventPage {
    pub events: Vec<AuditEventSummary>,
    pub next_cursor: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Cli,
    Web,
    Ide,
}

/// Short-lived, content-free indication that a client is viewing a Session.
/// Presence never carries a prompt, draft, token, editor contents, or Tool data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClientPresence {
    pub client_id: String,
    pub client_kind: ClientKind,
    pub actor_id: Id,
    pub device_id: Option<String>,
    pub session_id: Option<Id>,
    pub focused: bool,
    pub remote: bool,
    pub revocable: bool,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateClientPresence {
    pub scope: Scope,
    pub client_id: String,
    pub client_kind: ClientKind,
    pub session_id: Option<Id>,
    pub focused: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct RemoveClientPresence {
    pub scope: Scope,
    #[serde(default)]
    pub revoke_remote_grant: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamDashboardSummary {
    pub team_id: Id,
    pub active_goals: u64,
    pub ready_tasks: u64,
    pub in_progress_tasks: u64,
    pub blocked_tasks: u64,
    pub review_tasks: u64,
    pub verified_outcomes: u64,
    pub knowledge_items: u64,
    pub stale_knowledge_items: u64,
}

/// Content-free projection of effective signed Team governance. It exposes
/// data-handling labels, never the policy bundle, KMS key, or audit content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TeamGovernanceSummary {
    pub source: String,
    pub configuration_sequence: Option<u64>,
    pub policy_sequence: Option<u64>,
    pub issued_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub audit_content_enabled: bool,
    pub audit_retention_days: Option<u16>,
    pub data_residency_region: Option<String>,
    pub audit_content_categories: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamOwnership {
    pub id: Id,
    pub scope: Scope,
    pub resource_type: String,
    pub resource_uri: String,
    pub service_tier: Option<String>,
    pub on_call: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamOwnership {
    pub scope: Scope,
    pub resource_type: String,
    pub resource_uri: String,
    pub service_tier: Option<String>,
    pub on_call: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateTeamOwnership {
    pub scope: Scope,
    pub service_tier: Option<String>,
    pub on_call: Option<String>,
    pub new_team_id: Option<Id>,
    pub archived: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamCapacity {
    pub scope: Scope,
    pub human_available_hours: f64,
    pub agent_concurrency: u32,
    pub wip_limit: u32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateTeamCapacity {
    pub scope: Scope,
    pub human_available_hours: f64,
    pub agent_concurrency: u32,
    pub wip_limit: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct TeamBudget {
    pub id: Id,
    pub scope: Scope,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub model_limit_micros: u64,
    pub runner_limit_micros: u64,
    pub model_consumed_micros: u64,
    pub runner_consumed_micros: u64,
    pub model_reserved_micros: u64,
    pub runner_reserved_micros: u64,
    pub hard_limit: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateTeamBudget {
    pub scope: Scope,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub model_limit_micros: u64,
    pub runner_limit_micros: u64,
    pub hard_limit: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConsumeTeamBudget {
    pub scope: Scope,
    pub model_micros: u64,
    pub runner_micros: u64,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Proposed,
    AwaitingApproval,
    Running,
    Completed,
    Denied,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub request: ToolRequest,
    pub policy: PolicyResult,
    pub simulated_policy: Option<PolicyResult>,
    pub central_policy_applied: bool,
    pub team_configuration_sequence: Option<u64>,
    pub status: ToolCallStatus,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    Once,
    Session,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct Approval {
    pub id: Id,
    pub tool_call_id: Id,
    pub scope: Scope,
    pub approval_scope: ApprovalScope,
    pub status: ApprovalStatus,
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitToolCall {
    pub scope: Scope,
    pub tool: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
pub struct ResolveApproval {
    pub scope: Scope,
    pub approved: bool,
    pub approval_scope: ApprovalScope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveQuestion {
    pub scope: Scope,
    pub answers: Vec<QuestionAnswer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ToolCallOutcome {
    AwaitingApproval {
        tool_call: ToolCall,
        approval: Box<Approval>,
    },
    Denied {
        tool_call: ToolCall,
    },
    Completed {
        tool_call: ToolCall,
    },
    Failed {
        tool_call: ToolCall,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_requires_team() {
        let value = serde_json::json!({"organization_id":"org_1","actor_id":"usr_1"});
        assert!(serde_json::from_value::<Scope>(value).is_err());
    }
    #[test]
    fn unknown_capability_attributes_round_trip() {
        let manifest = CapabilityManifest {
            protocol_version: PROTOCOL_VERSION.into(),
            server_version: "0.1.0".into(),
            capabilities: vec![Capability {
                id: "runner.microvm".into(),
                version: "1".into(),
                maturity: CapabilityMaturity::Experimental,
                enabled: false,
                attributes: BTreeMap::from([("future".into(), serde_json::json!(true))]),
            }],
            contracts: deferred_capability_contracts(),
        };
        let encoded = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<CapabilityManifest>(&encoded).unwrap(),
            manifest
        );
    }

    #[test]
    fn capability_negotiation_rejects_protocol_major_mismatch_and_ignores_unknowns() {
        let manifest = CapabilityManifest {
            protocol_version: "1.7".into(),
            server_version: "2.0.0".into(),
            capabilities: vec![
                Capability {
                    id: "session.persistence".into(),
                    version: "1".into(),
                    maturity: CapabilityMaturity::Stable,
                    enabled: true,
                    attributes: BTreeMap::new(),
                },
                Capability {
                    id: "future.unknown".into(),
                    version: "99".into(),
                    maturity: CapabilityMaturity::Experimental,
                    enabled: true,
                    attributes: BTreeMap::new(),
                },
            ],
            contracts: vec![],
        };
        assert!(manifest.is_protocol_compatible("1.0"));
        assert!(!manifest.is_protocol_compatible("2.0"));
        assert!(manifest.supports("session.persistence", 1));
        assert!(!manifest.supports("future.unknown", 1));
        assert!(!manifest.supports("absent", 1));
    }

    #[test]
    fn every_deferred_capability_has_a_complete_extension_contract() {
        let contracts = deferred_capability_contracts();
        let expected = [
            "platform.windows_native",
            "runner.microvm",
            "session.realtime_multiuser",
            "task.durable",
            "agent.multi",
            "ide.jetbrains",
            "connector.enterprise",
            "extension.enterprise_runtime",
        ];
        assert_eq!(contracts.len(), expected.len());
        for id in expected {
            let contract = contracts
                .iter()
                .find(|item| item.capability_id == id)
                .unwrap();
            assert!(!contract.provider_interface.is_empty());
            assert!(!contract.input_schema.is_empty() && !contract.output_schema.is_empty());
            assert!(!contract.event_types.is_empty());
            assert!(!contract.permission_scopes.is_empty());
            assert!(!contract.policy_actions.is_empty());
            assert!(!contract.lifecycle.is_empty());
            assert!(!contract.cancellation_semantics.is_empty());
            assert!(!contract.recovery_semantics.is_empty());
            assert!(!contract.audit_events.is_empty());
            assert!(!contract.compatibility.is_empty());
            assert_eq!(contract.feature_flag, id);
        }
    }

    #[test]
    fn transcript_item_round_trips_with_stable_ownership() {
        let now = Utc::now();
        let item = TranscriptItem {
            id: Id("item_1".into()),
            session_id: Id("session_1".into()),
            turn_id: Id("turn_1".into()),
            kind: TranscriptItemKind::AgentMessage,
            status: TranscriptItemStatus::Completed,
            created_at: now,
            started_at: Some(now),
            completed_at: Some(now),
            summary: "Implemented the requested change".into(),
            content: TranscriptItemContent::Message {
                role: "assistant".into(),
                content: serde_json::json!("Implemented the requested change"),
                attachments: vec![],
            },
            detail: None,
            approval_id: None,
            policy_id: None,
            audit_event_id: None,
            truncated: false,
            retryable: true,
            cancellable: false,
            capability_version: "1".into(),
            revision: 1,
        };
        let encoded = serde_json::to_value(&item).unwrap();
        assert_eq!(encoded["session_id"], "session_1");
        assert_eq!(encoded["turn_id"], "turn_1");
        assert_eq!(encoded["kind"], "agent_message");
        assert_eq!(
            serde_json::from_value::<TranscriptItem>(encoded).unwrap(),
            item
        );
    }

    #[test]
    fn client_event_carries_projection_identity_and_version() {
        let event = ClientEvent {
            id: Id("event_1".into()),
            sequence: 7,
            timestamp: Utc::now(),
            session_id: Some(Id("session_1".into())),
            turn_id: Some(Id("turn_1".into())),
            item_id: Some(Id("item_1".into())),
            request_id: Some(Id("approval_1".into())),
            kind: "approval.required".into(),
            status: Some("awaiting_approval".into()),
            payload_version: 1,
            notification: Some(ClientNotification::ApprovalRequested {
                request_id: Id("approval_1".into()),
                item_id: Id("approval_1".into()),
                tool_item_id: Id("tool_1".into()),
                model_call_id: None,
                tool: "run_command".into(),
                summary: "Run command".into(),
            }),
            payload: serde_json::json!({"summary":"Run tests"}),
        };
        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["type"], "approval.required");
        assert_eq!(encoded["item_id"], "item_1");
        assert_eq!(encoded["request_id"], "approval_1");
        assert_eq!(encoded["payload_version"], 1);
        assert_eq!(encoded["notification"]["type"], "approval_requested");
    }
}
