use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use s_code_protocol::{GoalStatus, Id, PolicyDecision, PolicyResult, TeamTaskStatus, ToolRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub tool: String,
    pub decision: PolicyDecision,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyBundle {
    pub id: String,
    pub version: String,
    pub rules: Vec<Rule>,
    pub default: PolicyDecision,
}

impl Default for PolicyBundle {
    fn default() -> Self {
        Self {
            id: "builtin-safe-defaults".into(),
            version: "1".into(),
            rules: vec![
                Rule {
                    tool: "list_files".into(),
                    decision: PolicyDecision::Allow,
                    reason: "workspace file listing is allowed".into(),
                },
                Rule {
                    tool: "read_file".into(),
                    decision: PolicyDecision::Allow,
                    reason: "workspace reads are allowed".into(),
                },
                Rule {
                    tool: "search_text".into(),
                    decision: PolicyDecision::Allow,
                    reason: "workspace search is allowed".into(),
                },
                Rule {
                    tool: "tool_search".into(),
                    decision: PolicyDecision::Allow,
                    reason: "content-free extension Tool discovery is allowed".into(),
                },
                Rule {
                    tool: "apply_patch".into(),
                    decision: PolicyDecision::Ask,
                    reason: "writes require approval".into(),
                },
                Rule {
                    tool: "git_status".into(),
                    decision: PolicyDecision::Allow,
                    reason: "repository status is read-only".into(),
                },
                Rule {
                    tool: "git_diff".into(),
                    decision: PolicyDecision::Allow,
                    reason: "repository diff is read-only".into(),
                },
                Rule {
                    tool: "git_suggest_reviewers".into(),
                    decision: PolicyDecision::Allow,
                    reason: "CODEOWNERS and Team reviewer lookup is read-only".into(),
                },
                Rule {
                    tool: "git_create_branch".into(),
                    decision: PolicyDecision::Ask,
                    reason: "isolated branch creation requires approval".into(),
                },
                Rule {
                    tool: "git_commit".into(),
                    decision: PolicyDecision::Ask,
                    reason: "commits require approved diff evidence".into(),
                },
                Rule {
                    tool: "git_push".into(),
                    decision: PolicyDecision::Ask,
                    reason: "remote writes require approval".into(),
                },
                Rule {
                    tool: "scm_create_draft_pr".into(),
                    decision: PolicyDecision::Ask,
                    reason: "external SCM writes require explicit approval".into(),
                },
                Rule {
                    tool: "ticket_write_back".into(),
                    decision: PolicyDecision::Ask,
                    reason: "external work-management writes require explicit approval".into(),
                },
                Rule {
                    tool: "servicenow_read_record".into(),
                    decision: PolicyDecision::Allow,
                    reason: "bounded service-management record reads are allowed".into(),
                },
                Rule {
                    tool: "servicenow_append_work_note".into(),
                    decision: PolicyDecision::Ask,
                    reason: "ServiceNow writes require signed four-eyes and local approval".into(),
                },
                Rule {
                    tool: "chat_notify".into(),
                    decision: PolicyDecision::Ask,
                    reason: "external chat notifications require explicit approval".into(),
                },
                Rule {
                    tool: "ci_read_checks".into(),
                    decision: PolicyDecision::Allow,
                    reason: "bounded CI status reads are allowed".into(),
                },
                Rule {
                    tool: "siem_export".into(),
                    decision: PolicyDecision::Ask,
                    reason: "external SIEM writes require explicit approval".into(),
                },
                Rule {
                    tool: "git_force_push".into(),
                    decision: PolicyDecision::Deny,
                    reason: "force push is forbidden".into(),
                },
            ],
            default: PolicyDecision::Ask,
        }
    }
}

impl PolicyBundle {
    pub fn evaluate(&self, request: &ToolRequest) -> PolicyResult {
        let (decision, reason) = self
            .rules
            .iter()
            .find(|r| r.tool == request.tool)
            .map(|r| (r.decision.clone(), r.reason.clone()))
            .unwrap_or_else(|| {
                (
                    self.default.clone(),
                    "no matching rule; fail to approval".into(),
                )
            });
        PolicyResult {
            requires_approval: decision == PolicyDecision::Ask,
            decision,
            policy_id: self.id.clone(),
            policy_version: self.version.clone(),
            reason,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CentralPolicyPayload {
    pub organization_id: Id,
    pub team_id: Option<Id>,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub bundle: PolicyBundle,
    pub rollout_percent: u8,
    pub rollout_seed: String,
    pub simulate: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedPolicyBundle {
    pub key_id: String,
    pub payload: CentralPolicyPayload,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamAuditContentPolicy {
    /// Customer KMS/HSM key identifier used by GenerateDataKey/Decrypt. Raw
    /// key bytes are never part of the signed Team configuration.
    pub kms_key_id: String,
    pub retention_days: u16,
    pub residency_region: String,
    pub content_categories: Vec<String>,
}

impl TeamAuditContentPolicy {
    fn is_valid(&self) -> bool {
        let categories = self
            .content_categories
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        !self.kms_key_id.trim().is_empty()
            && self.kms_key_id.len() <= 256
            && (1..=3650).contains(&self.retention_days)
            && !self.residency_region.is_empty()
            && self.residency_region.len() <= 64
            && self
                .residency_region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && self.content_categories.len() == 1
            && categories.len() == 1
            && categories.contains("audit_event_payload")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamRuntimeConfiguration {
    pub human_available_hours: f64,
    pub agent_concurrency: u32,
    pub wip_limit: u32,
    pub allowed_model_ids: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_routing_order: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_fallback_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_content_policy: Option<TeamAuditContentPolicy>,
    pub policy_sequence: u64,
    pub policy_bundle: PolicyBundle,
    pub policy_rollout_percent: u8,
    pub policy_rollout_seed: String,
    pub policy_simulate: bool,
    pub knowledge_version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CentralTeamConfigurationPayload {
    pub organization_id: Id,
    pub team_id: Id,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub configuration: TeamRuntimeConfiguration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedTeamConfiguration {
    pub key_id: String,
    pub payload: CentralTeamConfigurationPayload,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplicatedTeamGoal {
    pub id: Id,
    pub title: String,
    pub outcome_definition: String,
    pub status: GoalStatus,
    pub target_date: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplicatedTeamTask {
    pub id: Id,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralTeamWorkPayload {
    pub organization_id: Id,
    pub team_id: Id,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub goals: Vec<ReplicatedTeamGoal>,
    pub tasks: Vec<ReplicatedTeamTask>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedTeamWorkSnapshot {
    pub key_id: String,
    pub payload: CentralTeamWorkPayload,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnterpriseExtensionKind {
    Plugin,
    Skill,
    McpServer,
    Connector,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnterpriseExtensionTool {
    pub name: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub data_classification: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnterpriseExtensionManifest {
    pub name: String,
    pub kind: EnterpriseExtensionKind,
    pub publisher: String,
    pub version: String,
    pub digest_sha256: String,
    pub source_uri: String,
    pub artifact_size_bytes: u64,
    pub artifact_media_type: String,
    pub runtime_protocol: String,
    pub runtime_arguments: Vec<String>,
    pub credential_environment: BTreeMap<String, String>,
    pub file_permissions: Vec<String>,
    pub network_permissions: Vec<String>,
    pub secret_permissions: Vec<String>,
    pub tools: Vec<EnterpriseExtensionTool>,
    pub supported_clients: Vec<String>,
    pub min_daemon_version: String,
    pub security_review: String,
    pub install_scope: String,
    pub auto_update: bool,
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnterpriseExtensionScanDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnterpriseExtensionSecurityAttestationPayload {
    pub organization_id: Id,
    pub team_id: Option<Id>,
    pub extension_id: Id,
    pub extension_sequence: u64,
    pub artifact_digest_sha256: String,
    pub artifact_size_bytes: u64,
    pub scanner: String,
    pub scanner_policy_version: String,
    pub sbom_digest_sha256: String,
    pub critical_findings: u32,
    pub high_findings: u32,
    pub medium_findings: u32,
    pub decision: EnterpriseExtensionScanDecision,
    pub scanned_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnterpriseExtensionSecurityAttestation {
    pub key_id: String,
    pub payload: EnterpriseExtensionSecurityAttestationPayload,
    pub signature: String,
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralExtensionManifestPayload {
    pub organization_id: Id,
    pub team_id: Option<Id>,
    pub extension_id: Id,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub manifest: EnterpriseExtensionManifest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedEnterpriseExtensionManifest {
    pub key_id: String,
    pub payload: CentralExtensionManifestPayload,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CentralPolicyExceptionPayload {
    pub organization_id: Id,
    pub team_id: Id,
    pub exception_id: Id,
    pub sequence: u64,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub active: bool,
    pub tool: String,
    pub actor_id: Option<Id>,
    pub workspace_uri: Option<String>,
    pub reason: String,
    pub requested_by: Id,
    pub approved_by: Id,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignedPolicyExceptionGrant {
    pub key_id: String,
    pub payload: CentralPolicyExceptionPayload,
    pub signature: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyVerificationError {
    #[error("policy signing key is malformed")]
    MalformedKey,
    #[error("unknown policy signing key")]
    UnknownKey,
    #[error("policy signature is malformed")]
    MalformedSignature,
    #[error("policy signature is invalid")]
    InvalidSignature,
    #[error("policy is outside its validity window")]
    OutsideValidity,
    #[error("policy rollout must be between 0 and 100")]
    InvalidRollout,
    #[error("policy sequence is not newer than the installed sequence")]
    RollbackAttempt,
    #[error("policy scope does not match the target organization/team")]
    ScopeMismatch,
    #[error("policy payload cannot be serialized")]
    Serialization,
    #[error("Team runtime configuration is invalid")]
    InvalidConfiguration,
}

#[derive(Clone, Default)]
pub struct PolicyTrustStore {
    keys: std::collections::BTreeMap<String, VerifyingKey>,
}

impl PolicyTrustStore {
    pub fn insert(&mut self, key_id: impl Into<String>, key: VerifyingKey) {
        self.keys.insert(key_id.into(), key);
    }

    pub fn insert_base64(
        &mut self,
        key_id: impl Into<String>,
        encoded: &str,
    ) -> Result<(), PolicyVerificationError> {
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| PolicyVerificationError::MalformedKey)?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| PolicyVerificationError::MalformedKey)?;
        let key =
            VerifyingKey::from_bytes(&bytes).map_err(|_| PolicyVerificationError::MalformedKey)?;
        self.insert(key_id, key);
        Ok(())
    }

    pub fn verify(
        &self,
        envelope: &SignedPolicyBundle,
        organization_id: &Id,
        team_id: Option<&Id>,
        installed_sequence: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<VerifiedPolicy, PolicyVerificationError> {
        let payload = &envelope.payload;
        if payload.organization_id != *organization_id || payload.team_id.as_ref() != team_id {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if payload.rollout_percent > 100 {
            return Err(PolicyVerificationError::InvalidRollout);
        }
        if now < payload.issued_at || now >= payload.expires_at {
            return Err(PolicyVerificationError::OutsideValidity);
        }
        if installed_sequence.is_some_and(|sequence| payload.sequence <= sequence) {
            return Err(PolicyVerificationError::RollbackAttempt);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(VerifiedPolicy(payload.clone()))
    }

    pub fn verify_team_configuration(
        &self,
        envelope: &SignedTeamConfiguration,
        organization_id: &Id,
        team_id: &Id,
        installed_sequence: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<CentralTeamConfigurationPayload, PolicyVerificationError> {
        let payload = &envelope.payload;
        if payload.organization_id != *organization_id || payload.team_id != *team_id {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if now < payload.issued_at || now >= payload.expires_at {
            return Err(PolicyVerificationError::OutsideValidity);
        }
        if installed_sequence.is_some_and(|sequence| payload.sequence <= sequence) {
            return Err(PolicyVerificationError::RollbackAttempt);
        }
        let config = &payload.configuration;
        let allowed_models = config
            .allowed_model_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let routes = config
            .model_routing_order
            .iter()
            .map(|id| id.0.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let fallback_reasons = config
            .model_fallback_reasons
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if !config.human_available_hours.is_finite()
            || config.human_available_hours < 0.0
            || config.agent_concurrency > 10_000
            || config.wip_limit == 0
            || config.wip_limit > 100_000
            || config.policy_rollout_percent > 100
            || config.policy_rollout_seed.trim().is_empty()
            || config.knowledge_version.trim().is_empty()
            || config.allowed_model_ids.len() > 64
            || allowed_models.len() != config.allowed_model_ids.len()
            || config.model_routing_order.len() > 64
            || routes.len() != config.model_routing_order.len()
            || config
                .model_routing_order
                .iter()
                .any(|id| !allowed_models.contains(id.0.as_str()))
            || fallback_reasons.len() != config.model_fallback_reasons.len()
            || config.model_fallback_reasons.iter().any(|reason| {
                !matches!(
                    reason.as_str(),
                    "rate_limited" | "timeout" | "provider_unavailable" | "context_overflow"
                )
            })
            || (!config.model_fallback_reasons.is_empty() && config.model_routing_order.len() < 2)
            || config
                .audit_content_policy
                .as_ref()
                .is_some_and(|policy| !policy.is_valid())
        {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(payload.clone())
    }

    pub fn verify_team_work_snapshot(
        &self,
        envelope: &SignedTeamWorkSnapshot,
        organization_id: &Id,
        team_id: &Id,
        installed_sequence: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<CentralTeamWorkPayload, PolicyVerificationError> {
        let payload = &envelope.payload;
        if payload.organization_id != *organization_id || payload.team_id != *team_id {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if now < payload.issued_at || now >= payload.expires_at {
            return Err(PolicyVerificationError::OutsideValidity);
        }
        if installed_sequence.is_some_and(|sequence| payload.sequence <= sequence) {
            return Err(PolicyVerificationError::RollbackAttempt);
        }
        if payload.goals.len() > 10_000 || payload.tasks.len() > 100_000 {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let goal_ids = payload
            .goals
            .iter()
            .map(|goal| goal.id.0.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let task_ids = payload
            .tasks
            .iter()
            .map(|task| task.id.0.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let invalid_goal = goal_ids.len() != payload.goals.len()
            || payload.goals.iter().any(|goal| {
                goal.id.0.trim().is_empty()
                    || goal.title.trim().is_empty()
                    || goal.outcome_definition.trim().is_empty()
                    || goal.status == GoalStatus::Achieved
            });
        let invalid_task = task_ids.len() != payload.tasks.len()
            || payload.tasks.iter().any(|task| {
                task.id.0.trim().is_empty()
                    || task.title.trim().is_empty()
                    || task.source.trim().is_empty()
                    || task.status == TeamTaskStatus::Verified
                    || (task.status == TeamTaskStatus::Blocked && task.blockers.is_empty())
                    || task
                        .goal_id
                        .as_ref()
                        .is_some_and(|id| !goal_ids.contains(id.0.as_str()))
            });
        if invalid_goal || invalid_task {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(payload.clone())
    }

    pub fn verify_extension_manifest(
        &self,
        envelope: &SignedEnterpriseExtensionManifest,
        organization_id: &Id,
        installed_sequence: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<CentralExtensionManifestPayload, PolicyVerificationError> {
        let payload = &envelope.payload;
        if payload.organization_id != *organization_id {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if now < payload.issued_at || now >= payload.expires_at {
            return Err(PolicyVerificationError::OutsideValidity);
        }
        if installed_sequence.is_some_and(|sequence| payload.sequence <= sequence) {
            return Err(PolicyVerificationError::RollbackAttempt);
        }
        let manifest = &payload.manifest;
        let digest_is_valid = manifest.digest_sha256.len() == 64
            && manifest
                .digest_sha256
                .bytes()
                .all(|value| value.is_ascii_hexdigit());
        let source_is_valid = ["https://", "oci://"]
            .iter()
            .any(|prefix| manifest.source_uri.starts_with(prefix));
        let bounded_permissions = manifest.file_permissions.len() <= 256
            && manifest.network_permissions.len() <= 256
            && manifest.secret_permissions.len() <= 256
            && manifest.tools.len() <= 512
            && manifest.supported_clients.len() <= 64
            && manifest.runtime_arguments.len() <= 128
            && manifest
                .runtime_arguments
                .iter()
                .all(|argument| argument.len() <= 4096 && !argument.contains('\0'))
            && manifest.credential_environment.len() <= 64;
        let valid_runtime = manifest.artifact_size_bytes > 0
            && manifest.artifact_size_bytes <= 512 * 1024 * 1024
            && manifest.artifact_media_type == "application/vnd.s-code.extension-executable.v1"
            && manifest.runtime_protocol == "mcp_stdio_v1"
            && manifest
                .credential_environment
                .iter()
                .all(|(name, handle)| {
                    valid_environment_name(name)
                        && valid_environment_name(handle)
                        && manifest.secret_permissions.contains(handle)
                });
        let valid_tools = manifest.tools.iter().all(|tool| {
            !tool.name.trim().is_empty()
                && tool.name.len() <= 256
                && !tool.data_classification.trim().is_empty()
                && tool.data_classification.len() <= 128
                && tool.input_schema.is_object()
                && tool.output_schema.is_object()
        });
        let install_scope_is_valid = match manifest.install_scope.as_str() {
            "organization" => payload.team_id.is_none(),
            "team" => payload
                .team_id
                .as_ref()
                .is_some_and(|team| !team.0.trim().is_empty()),
            _ => false,
        };
        if payload.extension_id.0.trim().is_empty()
            || manifest.name.trim().is_empty()
            || manifest.publisher.trim().is_empty()
            || manifest.version.trim().is_empty()
            || manifest.min_daemon_version.trim().is_empty()
            || manifest.security_review.trim().is_empty()
            || !install_scope_is_valid
            || !digest_is_valid
            || !source_is_valid
            || !bounded_permissions
            || !valid_runtime
            || !valid_tools
        {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(payload.clone())
    }

    pub fn verify_extension_security_attestation(
        &self,
        envelope: &SignedEnterpriseExtensionSecurityAttestation,
        manifest: &CentralExtensionManifestPayload,
        now: DateTime<Utc>,
    ) -> Result<EnterpriseExtensionSecurityAttestationPayload, PolicyVerificationError> {
        let payload = &envelope.payload;
        let valid_digest =
            |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if payload.organization_id != manifest.organization_id
            || payload.team_id != manifest.team_id
            || payload.extension_id != manifest.extension_id
        {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if payload.extension_sequence != manifest.sequence
            || !payload
                .artifact_digest_sha256
                .eq_ignore_ascii_case(&manifest.manifest.digest_sha256)
            || payload.artifact_size_bytes != manifest.manifest.artifact_size_bytes
            || now < payload.scanned_at
            || now >= payload.expires_at
            || payload.expires_at - payload.scanned_at > chrono::Duration::days(30)
            || payload.scanner.trim().is_empty()
            || payload.scanner.len() > 256
            || payload.scanner_policy_version.trim().is_empty()
            || payload.scanner_policy_version.len() > 128
            || !valid_digest(&payload.artifact_digest_sha256)
            || !valid_digest(&payload.sbom_digest_sha256)
            || payload.decision != EnterpriseExtensionScanDecision::Approved
            || payload.critical_findings != 0
            || payload.high_findings != 0
        {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(payload.clone())
    }

    pub fn verify_policy_exception(
        &self,
        envelope: &SignedPolicyExceptionGrant,
        organization_id: &Id,
        team_id: &Id,
        installed_sequence: Option<u64>,
        now: DateTime<Utc>,
    ) -> Result<CentralPolicyExceptionPayload, PolicyVerificationError> {
        let payload = &envelope.payload;
        if payload.organization_id != *organization_id || payload.team_id != *team_id {
            return Err(PolicyVerificationError::ScopeMismatch);
        }
        if now < payload.issued_at || now >= payload.expires_at {
            return Err(PolicyVerificationError::OutsideValidity);
        }
        if installed_sequence.is_some_and(|sequence| payload.sequence <= sequence) {
            return Err(PolicyVerificationError::RollbackAttempt);
        }
        if payload.exception_id.0.trim().is_empty()
            || payload.tool.trim().is_empty()
            || payload.reason.trim().is_empty()
            || payload.requested_by == payload.approved_by
            || payload.expires_at - payload.issued_at > chrono::Duration::days(30)
            || payload
                .workspace_uri
                .as_ref()
                .is_some_and(|value| value.trim().is_empty())
        {
            return Err(PolicyVerificationError::InvalidConfiguration);
        }
        let key = self
            .keys
            .get(&envelope.key_id)
            .ok_or(PolicyVerificationError::UnknownKey)?;
        let signature_bytes = STANDARD
            .decode(&envelope.signature)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| PolicyVerificationError::MalformedSignature)?;
        let canonical =
            serde_json::to_vec(payload).map_err(|_| PolicyVerificationError::Serialization)?;
        key.verify(&canonical, &signature)
            .map_err(|_| PolicyVerificationError::InvalidSignature)?;
        Ok(payload.clone())
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedPolicy(CentralPolicyPayload);

impl VerifiedPolicy {
    pub fn payload(&self) -> &CentralPolicyPayload {
        &self.0
    }

    pub fn applies_to_device(&self, device_id: &Id) -> bool {
        applies_to_rollout(self.0.rollout_percent, &self.0.rollout_seed, device_id)
    }
}

#[derive(Clone, Debug)]
pub struct LayeredPolicyResult {
    pub effective: PolicyResult,
    pub simulated: Option<PolicyResult>,
    pub central_applied: bool,
}

/// Evaluate local safety controls before a verified central bundle. A local
/// deny is mandatory and cannot be weakened by rollout, simulation or a
/// centrally supplied allow.
pub fn evaluate_layered(
    local: &PolicyBundle,
    central: Option<&VerifiedPolicy>,
    device_id: &Id,
    request: &ToolRequest,
) -> LayeredPolicyResult {
    let local_result = local.evaluate(request);
    if local_result.decision == PolicyDecision::Deny {
        return LayeredPolicyResult {
            effective: local_result,
            simulated: None,
            central_applied: false,
        };
    }
    let Some(central) = central.filter(|policy| policy.applies_to_device(device_id)) else {
        return LayeredPolicyResult {
            effective: local_result,
            simulated: None,
            central_applied: false,
        };
    };
    let central_result = central.payload().bundle.evaluate(request);
    if central.payload().simulate {
        LayeredPolicyResult {
            effective: local_result,
            simulated: Some(central_result),
            central_applied: false,
        }
    } else {
        LayeredPolicyResult {
            effective: central_result,
            simulated: None,
            central_applied: true,
        }
    }
}

pub fn applies_to_rollout(percent: u8, seed: &str, device_id: &Id) -> bool {
    if percent == 100 {
        return true;
    }
    if percent == 0 {
        return false;
    }
    let mut digest = Sha256::new();
    digest.update(seed.as_bytes());
    digest.update([0]);
    digest.update(device_id.0.as_bytes());
    let bytes = digest.finalize();
    u16::from_be_bytes([bytes[0], bytes[1]]) % 100 < u16::from(percent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use ed25519_dalek::{Signer, SigningKey};
    use s_code_protocol::{Id, Scope};
    fn request(tool: &str) -> ToolRequest {
        ToolRequest {
            parent_tool_call_id: None,
            id: Id::new("tool"),
            scope: Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("user".into()),
                goal_id: None,
                task_id: None,
            },
            session_id: Id("session".into()),
            turn_id: Id("turn".into()),
            tool: tool.into(),
            arguments: serde_json::json!({}),
            created_at: Utc::now(),
        }
    }
    #[test]
    fn unknown_tools_require_approval() {
        assert_eq!(
            PolicyBundle::default()
                .evaluate(&request("future"))
                .decision,
            PolicyDecision::Ask
        );
    }
    #[test]
    fn force_push_is_denied() {
        assert_eq!(
            PolicyBundle::default()
                .evaluate(&request("git_force_push"))
                .decision,
            PolicyDecision::Deny
        );
    }

    fn signed_policy(
        bundle: PolicyBundle,
        simulate: bool,
    ) -> (PolicyTrustStore, SignedPolicyBundle) {
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let payload = CentralPolicyPayload {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            sequence: 2,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::hours(1),
            bundle,
            rollout_percent: 100,
            rollout_seed: "deployment-a".into(),
            simulate,
        };
        let signature = signing.sign(&serde_json::to_vec(&payload).unwrap());
        let mut trust = PolicyTrustStore::default();
        trust.insert("root-1", signing.verifying_key());
        (
            trust,
            SignedPolicyBundle {
                key_id: "root-1".into(),
                payload,
                signature: STANDARD.encode(signature.to_bytes()),
            },
        )
    }

    #[test]
    fn signed_policy_is_scope_time_and_sequence_bound() {
        let (trust, envelope) = signed_policy(PolicyBundle::default(), false);
        assert!(
            trust
                .verify(
                    &envelope,
                    &Id("org".into()),
                    Some(&Id("team".into())),
                    Some(1),
                    Utc::now(),
                )
                .is_ok()
        );
        assert!(matches!(
            trust.verify(
                &envelope,
                &Id("other".into()),
                Some(&Id("team".into())),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::ScopeMismatch)
        ));
        assert!(matches!(
            trust.verify(
                &envelope,
                &Id("org".into()),
                Some(&Id("team".into())),
                Some(2),
                Utc::now(),
            ),
            Err(PolicyVerificationError::RollbackAttempt)
        ));
    }

    #[test]
    fn tampering_invalidates_policy_signature() {
        let (trust, mut envelope) = signed_policy(PolicyBundle::default(), false);
        envelope.payload.bundle.default = PolicyDecision::Allow;
        assert!(matches!(
            trust.verify(
                &envelope,
                &Id("org".into()),
                Some(&Id("team".into())),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidSignature)
        ));
    }

    #[test]
    fn signed_team_configuration_is_scope_version_time_and_content_bound() {
        let signing = SigningKey::from_bytes(&[9_u8; 32]);
        let payload = CentralTeamConfigurationPayload {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            sequence: 4,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::hours(1),
            configuration: TeamRuntimeConfiguration {
                human_available_hours: 32.0,
                agent_concurrency: 3,
                wip_limit: 5,
                allowed_model_ids: vec![Id("model-1".into())],
                model_routing_order: vec![Id("model-1".into())],
                model_fallback_reasons: Vec::new(),
                policy_sequence: 7,
                policy_bundle: PolicyBundle::default(),
                policy_rollout_percent: 100,
                policy_rollout_seed: "all-devices".into(),
                policy_simulate: false,
                knowledge_version: "knowledge-3".into(),
                audit_content_policy: None,
            },
        };
        let mut envelope = SignedTeamConfiguration {
            key_id: "root-1".into(),
            signature: STANDARD.encode(
                signing
                    .sign(&serde_json::to_vec(&payload).unwrap())
                    .to_bytes(),
            ),
            payload: payload.clone(),
        };
        let mut trust = PolicyTrustStore::default();
        trust.insert("root-1", signing.verifying_key());
        assert!(
            trust
                .verify_team_configuration(
                    &envelope,
                    &Id("org".into()),
                    &Id("team".into()),
                    Some(3),
                    Utc::now(),
                )
                .is_ok()
        );
        envelope.payload.configuration.audit_content_policy = Some(TeamAuditContentPolicy {
            kms_key_id: "kms/org/team/audit-content".into(),
            retention_days: 30,
            residency_region: "us-west-2".into(),
            content_categories: vec!["audit_event_payload".into()],
        });
        envelope.signature = STANDARD.encode(
            signing
                .sign(&serde_json::to_vec(&envelope.payload).unwrap())
                .to_bytes(),
        );
        assert!(
            trust
                .verify_team_configuration(
                    &envelope,
                    &Id("org".into()),
                    &Id("team".into()),
                    Some(3),
                    Utc::now(),
                )
                .is_ok()
        );
        envelope
            .payload
            .configuration
            .audit_content_policy
            .as_mut()
            .unwrap()
            .retention_days = 0;
        assert!(matches!(
            trust.verify_team_configuration(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidConfiguration)
        ));
        envelope.payload.configuration.audit_content_policy = None;
        assert!(matches!(
            trust.verify_team_configuration(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                Some(4),
                Utc::now(),
            ),
            Err(PolicyVerificationError::RollbackAttempt)
        ));
        envelope.payload.configuration.wip_limit = 0;
        assert!(matches!(
            trust.verify_team_configuration(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidConfiguration)
        ));
        envelope.payload.configuration.wip_limit = 5;
        envelope.payload.configuration.model_fallback_reasons = vec!["timeout".into()];
        assert!(matches!(
            trust.verify_team_configuration(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidConfiguration)
        ));
    }

    #[test]
    fn signed_team_work_snapshot_rejects_replay_tampering_and_unverified_terminal_state() {
        let signing = SigningKey::from_bytes(&[10_u8; 32]);
        let payload = CentralTeamWorkPayload {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            sequence: 2,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::hours(1),
            goals: vec![ReplicatedTeamGoal {
                id: Id("goal-central".into()),
                title: "Ship safely".into(),
                outcome_definition: "Verified production outcome".into(),
                status: GoalStatus::Active,
                target_date: None,
            }],
            tasks: vec![ReplicatedTeamTask {
                id: Id("task-central".into()),
                goal_id: Some(Id("goal-central".into())),
                source: "control-plane".into(),
                title: "Implement feature".into(),
                priority: 100,
                assignee_type: Some("agent".into()),
                assignee_id: None,
                status: TeamTaskStatus::Ready,
                acceptance_criteria: vec!["tests pass".into()],
                required_evidence: vec!["test".into()],
                blockers: vec![],
            }],
        };
        let mut envelope = SignedTeamWorkSnapshot {
            key_id: "root-1".into(),
            signature: STANDARD.encode(
                signing
                    .sign(&serde_json::to_vec(&payload).unwrap())
                    .to_bytes(),
            ),
            payload,
        };
        let mut trust = PolicyTrustStore::default();
        trust.insert("root-1", signing.verifying_key());
        assert!(
            trust
                .verify_team_work_snapshot(
                    &envelope,
                    &Id("org".into()),
                    &Id("team".into()),
                    Some(1),
                    Utc::now(),
                )
                .is_ok()
        );
        assert_eq!(
            trust.verify_team_work_snapshot(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                Some(2),
                Utc::now(),
            ),
            Err(PolicyVerificationError::RollbackAttempt)
        );
        envelope.payload.tasks[0].status = TeamTaskStatus::Verified;
        assert_eq!(
            trust.verify_team_work_snapshot(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidConfiguration)
        );
    }

    #[test]
    fn signed_policy_exception_is_two_person_scoped_limited_and_replay_safe() {
        let signing = SigningKey::from_bytes(&[13_u8; 32]);
        let payload = CentralPolicyExceptionPayload {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            exception_id: Id("exception-1".into()),
            sequence: 1,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::hours(1),
            active: true,
            tool: "git_push".into(),
            actor_id: Some(Id("requester".into())),
            workspace_uri: Some("file:///repo".into()),
            reason: "approved release window".into(),
            requested_by: Id("requester".into()),
            approved_by: Id("approver".into()),
        };
        let mut envelope = SignedPolicyExceptionGrant {
            key_id: "root-1".into(),
            signature: STANDARD.encode(
                signing
                    .sign(&serde_json::to_vec(&payload).unwrap())
                    .to_bytes(),
            ),
            payload,
        };
        let mut trust = PolicyTrustStore::default();
        trust.insert("root-1", signing.verifying_key());
        assert!(
            trust
                .verify_policy_exception(
                    &envelope,
                    &Id("org".into()),
                    &Id("team".into()),
                    None,
                    Utc::now(),
                )
                .is_ok()
        );
        assert_eq!(
            trust.verify_policy_exception(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                Some(1),
                Utc::now(),
            ),
            Err(PolicyVerificationError::RollbackAttempt)
        );
        envelope.payload.approved_by = envelope.payload.requested_by.clone();
        assert_eq!(
            trust.verify_policy_exception(
                &envelope,
                &Id("org".into()),
                &Id("team".into()),
                None,
                Utc::now(),
            ),
            Err(PolicyVerificationError::InvalidConfiguration)
        );
    }

    #[test]
    fn signed_enterprise_extension_manifest_is_validated_and_replay_safe() {
        let signing = SigningKey::from_bytes(&[17_u8; 32]);
        let payload = CentralExtensionManifestPayload {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            extension_id: Id("skill-secure-review".into()),
            sequence: 3,
            issued_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::days(30),
            manifest: EnterpriseExtensionManifest {
                name: "Secure Review".into(),
                kind: EnterpriseExtensionKind::Skill,
                publisher: "acme-security".into(),
                version: "1.2.0".into(),
                digest_sha256: "a".repeat(64),
                source_uri: "https://registry.example/secure-review.tgz".into(),
                artifact_size_bytes: 1024,
                artifact_media_type: "application/vnd.s-code.extension-executable.v1".into(),
                runtime_protocol: "mcp_stdio_v1".into(),
                runtime_arguments: vec!["--stdio".into()],
                credential_environment: BTreeMap::new(),
                file_permissions: vec!["workspace.read".into()],
                network_permissions: vec![],
                secret_permissions: vec![],
                tools: vec![EnterpriseExtensionTool {
                    name: "review".into(),
                    input_schema: serde_json::json!({"type":"object"}),
                    output_schema: serde_json::json!({"type":"object"}),
                    data_classification: "source_code".into(),
                }],
                supported_clients: vec![
                    "daemon>=0.1.0".into(),
                    "vscode>=0.1".into(),
                    "cli>=0.1".into(),
                ],
                min_daemon_version: "0.1.0".into(),
                security_review: "approved:SEC-42".into(),
                install_scope: "team".into(),
                auto_update: false,
                revoked: false,
            },
        };
        let mut envelope = SignedEnterpriseExtensionManifest {
            key_id: "root-1".into(),
            signature: STANDARD.encode(
                signing
                    .sign(&serde_json::to_vec(&payload).unwrap())
                    .to_bytes(),
            ),
            payload: payload.clone(),
        };
        let mut trust = PolicyTrustStore::default();
        trust.insert("root-1", signing.verifying_key());
        assert!(
            trust
                .verify_extension_manifest(&envelope, &Id("org".into()), Some(2), Utc::now())
                .is_ok()
        );
        let scan_payload = EnterpriseExtensionSecurityAttestationPayload {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            extension_id: Id("skill-secure-review".into()),
            extension_sequence: 3,
            artifact_digest_sha256: "a".repeat(64),
            artifact_size_bytes: 1024,
            scanner: "enterprise-scanner".into(),
            scanner_policy_version: "2026-07".into(),
            sbom_digest_sha256: "c".repeat(64),
            critical_findings: 0,
            high_findings: 0,
            medium_findings: 1,
            decision: EnterpriseExtensionScanDecision::Approved,
            scanned_at: Utc::now() - Duration::minutes(1),
            expires_at: Utc::now() + Duration::days(7),
        };
        let mut scan = SignedEnterpriseExtensionSecurityAttestation {
            key_id: "root-1".into(),
            signature: STANDARD.encode(
                signing
                    .sign(&serde_json::to_vec(&scan_payload).unwrap())
                    .to_bytes(),
            ),
            payload: scan_payload,
        };
        assert!(
            trust
                .verify_extension_security_attestation(&scan, &payload, Utc::now())
                .is_ok()
        );
        scan.payload.high_findings = 1;
        assert_eq!(
            trust.verify_extension_security_attestation(&scan, &payload, Utc::now()),
            Err(PolicyVerificationError::InvalidConfiguration)
        );
        assert_eq!(
            trust.verify_extension_manifest(&envelope, &Id("org".into()), Some(3), Utc::now()),
            Err(PolicyVerificationError::RollbackAttempt)
        );
        envelope.payload.manifest.network_permissions = vec!["*".into()];
        assert_eq!(
            trust.verify_extension_manifest(&envelope, &Id("org".into()), None, Utc::now()),
            Err(PolicyVerificationError::InvalidSignature)
        );
    }

    #[test]
    fn local_mandatory_deny_cannot_be_overridden() {
        let central = PolicyBundle {
            id: "central".into(),
            version: "2".into(),
            rules: vec![Rule {
                tool: "git_force_push".into(),
                decision: PolicyDecision::Allow,
                reason: "attempted override".into(),
            }],
            default: PolicyDecision::Allow,
        };
        let (trust, envelope) = signed_policy(central, false);
        let verified = trust
            .verify(
                &envelope,
                &Id("org".into()),
                Some(&Id("team".into())),
                None,
                Utc::now(),
            )
            .unwrap();
        let result = evaluate_layered(
            &PolicyBundle::default(),
            Some(&verified),
            &Id("device".into()),
            &request("git_force_push"),
        );
        assert_eq!(result.effective.decision, PolicyDecision::Deny);
        assert!(!result.central_applied);
    }

    #[test]
    fn simulation_never_changes_effective_decision() {
        let central = PolicyBundle {
            id: "central".into(),
            version: "2".into(),
            rules: vec![],
            default: PolicyDecision::Deny,
        };
        let (trust, envelope) = signed_policy(central, true);
        let verified = trust
            .verify(
                &envelope,
                &Id("org".into()),
                Some(&Id("team".into())),
                None,
                Utc::now(),
            )
            .unwrap();
        let result = evaluate_layered(
            &PolicyBundle::default(),
            Some(&verified),
            &Id("device".into()),
            &request("read_file"),
        );
        assert_eq!(result.effective.decision, PolicyDecision::Allow);
        assert_eq!(result.simulated.unwrap().decision, PolicyDecision::Deny);
    }

    #[test]
    fn rollout_assignment_is_stable_and_has_fail_safe_boundaries() {
        let device = Id("device-a".into());
        assert!(!applies_to_rollout(0, "release-1", &device));
        assert!(applies_to_rollout(100, "release-1", &device));
        let assigned = applies_to_rollout(37, "release-1", &device);
        assert_eq!(assigned, applies_to_rollout(37, "release-1", &device));
    }
}
