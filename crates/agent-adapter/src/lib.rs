use async_trait::async_trait;
use chrono::{DateTime, Utc};
use s_code_protocol::Scope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_ID: usize = 128;
const MAX_METADATA: usize = 32;
const MAX_ARTIFACTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceLevel {
    Observe,
    Integrate,
    Enforce,
    ManagedExecution,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementPoint {
    Tool,
    Network,
    Credential,
    Execution,
}

impl EnforcementPoint {
    pub fn all() -> BTreeSet<Self> {
        [Self::Tool, Self::Network, Self::Credential, Self::Execution]
            .into_iter()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDescriptor {
    pub schema_version: u32,
    pub adapter_id: String,
    pub adapter_version: String,
    pub protocol_version: String,
    pub governance_level: GovernanceLevel,
    pub enforcement_points: BTreeSet<EnforcementPoint>,
    pub fail_closed: bool,
    pub managed_execution_plane: bool,
    pub supported_event_kinds: BTreeSet<String>,
}

impl AdapterDescriptor {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.schema_version != 1
            || !safe_id(&self.adapter_id)
            || !safe_version(&self.adapter_version)
            || self.protocol_version != "1"
            || self.supported_event_kinds.is_empty()
            || self.supported_event_kinds.len() > 64
            || self
                .supported_event_kinds
                .iter()
                .any(|value| !safe_id(value))
        {
            return Err(AdapterError::InvalidDescriptor);
        }
        match self.governance_level {
            GovernanceLevel::Observe | GovernanceLevel::Integrate => {
                if !self.enforcement_points.is_empty()
                    || self.fail_closed
                    || self.managed_execution_plane
                {
                    return Err(AdapterError::OverstatedGovernance);
                }
            }
            GovernanceLevel::Enforce => {
                if self.enforcement_points.is_empty()
                    || !self.fail_closed
                    || self.managed_execution_plane
                {
                    return Err(AdapterError::OverstatedGovernance);
                }
            }
            GovernanceLevel::ManagedExecution => {
                if self.enforcement_points != EnforcementPoint::all()
                    || !self.fail_closed
                    || !self.managed_execution_plane
                {
                    return Err(AdapterError::OverstatedGovernance);
                }
            }
        }
        Ok(())
    }

    pub fn guarantee(&self) -> &'static str {
        match self.governance_level {
            GovernanceLevel::Observe => "observable_only_no_blocking_guarantee",
            GovernanceLevel::Integrate => "shared_context_no_complete_blocking_guarantee",
            GovernanceLevel::Enforce => "blocking_only_at_declared_enforcement_points",
            GovernanceLevel::ManagedExecution => "shared_execution_plane_security_kernel",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedAgentEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub scope: Scope,
    pub adapter_id: String,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub node_id: String,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub payload_sha256: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl NormalizedAgentEvent {
    pub fn validate(&self, descriptor: &AdapterDescriptor) -> Result<(), AdapterError> {
        descriptor.validate()?;
        if self.schema_version != 1
            || self.adapter_id != descriptor.adapter_id
            || !safe_id(&self.event_id)
            || !safe_id(&self.run_id)
            || self
                .parent_run_id
                .as_ref()
                .is_some_and(|value| !safe_id(value))
            || !safe_id(&self.node_id)
            || !descriptor.supported_event_kinds.contains(&self.kind)
            || !is_sha256(&self.payload_sha256)
            || self.metadata.len() > MAX_METADATA
        {
            return Err(AdapterError::InvalidEvent);
        }
        for (key, value) in &self.metadata {
            let lower = key.to_ascii_lowercase();
            if !safe_id(key)
                || value.len() > 256
                || [
                    "content", "prompt", "source", "stdout", "stderr", "secret", "token",
                ]
                .iter()
                .any(|forbidden| lower.contains(forbidden))
            {
                return Err(AdapterError::ContentBearingMetadata);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub uri: String,
    pub revision: u64,
    pub digest_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInvocation {
    pub schema_version: u32,
    pub scope: Scope,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub node_id: String,
    pub idempotency_key: String,
    pub action: EnforcementPoint,
    /// Local execution payload. Central normalized events carry only its digest.
    pub request: Value,
    pub request_sha256: String,
    pub budget_micros: u64,
    pub inputs: Vec<ArtifactRef>,
}

impl AgentInvocation {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.schema_version != 1
            || !safe_id(&self.run_id)
            || self
                .parent_run_id
                .as_ref()
                .is_some_and(|value| !safe_id(value))
            || !safe_id(&self.node_id)
            || !safe_id(&self.idempotency_key)
            || !self.request.is_object()
            || serde_json::to_vec(&self.request).map_or(true, |value| value.len() > 256 * 1024)
            || payload_sha256(&self.request).as_deref() != Ok(self.request_sha256.as_str())
            || self.budget_micros == 0
            || self.inputs.len() > MAX_ARTIFACTS
            || self
                .inputs
                .iter()
                .any(|artifact| artifact.validate().is_err())
        {
            return Err(AdapterError::InvalidInvocation);
        }
        Ok(())
    }
}

impl ArtifactRef {
    pub fn validate(&self) -> Result<(), AdapterError> {
        if self.uri.len() > 512
            || !(self.uri.starts_with("artifact://") || self.uri.starts_with("git://"))
            || !is_sha256(&self.digest_sha256)
        {
            return Err(AdapterError::InvalidArtifact);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentInvocationStatus {
    Completed,
    AwaitingApproval,
    Rejected,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInvocationResult {
    pub schema_version: u32,
    pub run_id: String,
    pub node_id: String,
    pub idempotency_key: String,
    pub status: AgentInvocationStatus,
    pub usage_micros: u64,
    pub artifacts: Vec<ArtifactRef>,
    pub result_sha256: String,
}

impl AgentInvocationResult {
    pub fn validate_for(&self, input: &AgentInvocation) -> Result<(), AdapterError> {
        if self.schema_version != 1
            || self.run_id != input.run_id
            || self.node_id != input.node_id
            || self.idempotency_key != input.idempotency_key
            || self.usage_micros > input.budget_micros
            || self.artifacts.len() > MAX_ARTIFACTS
            || self
                .artifacts
                .iter()
                .any(|artifact| artifact.validate().is_err())
            || !is_sha256(&self.result_sha256)
        {
            return Err(AdapterError::InvalidResult);
        }
        Ok(())
    }
}

#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    async fn invoke(
        &self,
        invocation: AgentInvocation,
    ) -> Result<AgentInvocationResult, AdapterError>;
}

pub fn validate_enforced_invocation(
    descriptor: &AdapterDescriptor,
    invocation: &AgentInvocation,
) -> Result<(), AdapterError> {
    descriptor.validate()?;
    invocation.validate()?;
    if descriptor.governance_level < GovernanceLevel::Enforce
        || !descriptor.fail_closed
        || !descriptor.enforcement_points.contains(&invocation.action)
    {
        return Err(AdapterError::EnforcementUnavailable);
    }
    Ok(())
}

pub fn payload_sha256(value: &Value) -> Result<String, AdapterError> {
    let encoded = serde_json::to_vec(value).map_err(|_| AdapterError::InvalidInvocation)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdapterError {
    #[error("invalid adapter descriptor")]
    InvalidDescriptor,
    #[error("adapter governance claim exceeds its declared enforcement")]
    OverstatedGovernance,
    #[error("invalid normalized agent event")]
    InvalidEvent,
    #[error("agent event metadata may not contain content-bearing fields")]
    ContentBearingMetadata,
    #[error("invalid agent invocation")]
    InvalidInvocation,
    #[error("invalid artifact reference")]
    InvalidArtifact,
    #[error("invalid agent invocation result")]
    InvalidResult,
    #[error("adapter cannot enforce this invocation path")]
    EnforcementUnavailable,
    #[error("adapter execution failed: {0}")]
    Execution(String),
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn safe_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use s_code_protocol::Id;

    fn descriptor(level: GovernanceLevel) -> AdapterDescriptor {
        AdapterDescriptor {
            schema_version: 1,
            adapter_id: "codex-reference".into(),
            adapter_version: "1.0.0".into(),
            protocol_version: "1".into(),
            governance_level: level,
            enforcement_points: BTreeSet::new(),
            fail_closed: false,
            managed_execution_plane: false,
            supported_event_kinds: BTreeSet::from(["agent.completed".into()]),
        }
    }

    fn scope() -> Scope {
        Scope {
            organization_id: Id("org-a".into()),
            team_id: Id("team-a".into()),
            actor_id: Id("agent-a".into()),
            goal_id: None,
            task_id: Some(Id("task-a".into())),
        }
    }

    #[test]
    fn governance_levels_cannot_overstate_their_enforcement() {
        assert!(descriptor(GovernanceLevel::Observe).validate().is_ok());
        let mut observe = descriptor(GovernanceLevel::Observe);
        observe.enforcement_points.insert(EnforcementPoint::Tool);
        observe.fail_closed = true;
        assert_eq!(observe.validate(), Err(AdapterError::OverstatedGovernance));

        let mut enforce = descriptor(GovernanceLevel::Enforce);
        assert_eq!(enforce.validate(), Err(AdapterError::OverstatedGovernance));
        enforce.enforcement_points.insert(EnforcementPoint::Tool);
        enforce.fail_closed = true;
        assert!(enforce.validate().is_ok());

        let mut managed = descriptor(GovernanceLevel::ManagedExecution);
        managed.enforcement_points = EnforcementPoint::all();
        managed.fail_closed = true;
        managed.managed_execution_plane = true;
        assert!(managed.validate().is_ok());
    }

    #[test]
    fn normalized_events_are_scope_bound_and_content_free() {
        let descriptor = descriptor(GovernanceLevel::Observe);
        let mut event = NormalizedAgentEvent {
            schema_version: 1,
            event_id: "event-1".into(),
            scope: scope(),
            adapter_id: descriptor.adapter_id.clone(),
            run_id: "run-1".into(),
            parent_run_id: None,
            node_id: "node-1".into(),
            kind: "agent.completed".into(),
            occurred_at: Utc::now(),
            payload_sha256: "a".repeat(64),
            metadata: BTreeMap::from([("model_id".into(), "model-a".into())]),
        };
        assert!(event.validate(&descriptor).is_ok());
        event
            .metadata
            .insert("prompt_content".into(), "secret source".into());
        assert_eq!(
            event.validate(&descriptor),
            Err(AdapterError::ContentBearingMetadata)
        );
    }

    #[test]
    fn only_declared_fail_closed_paths_can_be_called_enforced() {
        let invocation = AgentInvocation {
            schema_version: 1,
            scope: scope(),
            run_id: "run-1".into(),
            parent_run_id: None,
            node_id: "node-1".into(),
            idempotency_key: "run-1-node-1".into(),
            action: EnforcementPoint::Credential,
            request: serde_json::json!({"operation":"read"}),
            request_sha256: payload_sha256(&serde_json::json!({"operation":"read"})).unwrap(),
            budget_micros: 100,
            inputs: vec![],
        };
        let mut enforce = descriptor(GovernanceLevel::Enforce);
        enforce.enforcement_points.insert(EnforcementPoint::Tool);
        enforce.fail_closed = true;
        assert_eq!(
            validate_enforced_invocation(&enforce, &invocation),
            Err(AdapterError::EnforcementUnavailable)
        );
        enforce
            .enforcement_points
            .insert(EnforcementPoint::Credential);
        assert!(validate_enforced_invocation(&enforce, &invocation).is_ok());
        let mut tampered = invocation;
        tampered.request = serde_json::json!({"operation":"write"});
        assert_eq!(
            validate_enforced_invocation(&enforce, &tampered),
            Err(AdapterError::InvalidInvocation)
        );
    }
}
