use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use opencoding_audit::SignedCentralAuditBatch;
use opencoding_policy::{
    CentralPolicyExceptionPayload, SignedPolicyExceptionGrant, SignedTeamConfiguration,
    SignedTeamWorkSnapshot,
};
use opencoding_protocol::{
    Approval, ApprovalScope, ApprovalStatus, Artifact, Attachment, AttachmentMetadata,
    BackgroundTerminalSpec, BackgroundTerminalStatus, BackgroundTerminalSummary, ConsumeTeamBudget,
    ContinueTeamGoal, CreateAttachment, CreateDurableTask, CreateSession, CreateTeamBudget,
    CreateTeamGoal, CreateTeamKnowledgeItem, CreateTeamOutcome, CreateTeamOwnership,
    CreateTeamTask, CreateTurnInput, DaemonSettings, DurableTask, DurableTaskStatus, EditorContext,
    Event, GoalStatus, HookInstallation, HookSpec, Id, MarketplaceInstallation, MarketplaceSource,
    McpHttpInstallation, McpHttpServerSpec, McpInstallation, McpServerSpec, Message,
    OutcomeEvidence, PermissionMode, PluginBundle, PluginInstallation, PolicyDecision,
    PolicyResult, QuestionAnswer, QuestionRequest, QuestionStatus, Scope, Session,
    SessionForkMetadata, SessionGoal, SessionGoalStatus, SessionPreferences, SessionStatus,
    SessionUsage, SetSessionGoal, SideConversation, SideConversationStatus, SkillInstallation,
    SkillSpec, TeamBudget, TeamCapacity, TeamDashboardSummary, TeamGoal, TeamGoalRun,
    TeamGoalRunStatus, TeamKnowledgeItem, TeamOutcome, TeamOwnership, TeamTask, TeamTaskStatus,
    TeamWorkSyncResult, ToolCall, ToolCallStatus, ToolRequest, Turn, TurnInput, TurnInputMode,
    TurnInputStatus, TurnStatus, UpdateEditorContext, UpdateSessionGoal, UpdateSessionPreferences,
    UpdateTeamCapacity, UpdateTeamGoal, UpdateTeamOwnership, UpdateTeamTask,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{
    QueryBuilder, Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("invalid stored data: {0}")]
    InvalidData(String),
    #[error("resource not found")]
    NotFound,
    #[error("scope mismatch")]
    ScopeMismatch,
    #[error("invalid state transition: {0}")]
    InvalidState(String),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sensitive data encryption failed: {0}")]
    Encryption(String),
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    sensitive: SensitiveCodec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageFormat {
    Empty,
    Unmarked,
    Marked { key_id: String },
}

#[derive(Clone, Default)]
enum SensitiveCodec {
    #[default]
    PlaintextDevelopment,
    Aes256Gcm {
        key_id: String,
        cipher: Box<Aes256Gcm>,
    },
}

impl SensitiveCodec {
    fn key_id(&self) -> &str {
        match self {
            Self::PlaintextDevelopment => "plaintext-v1",
            Self::Aes256Gcm { key_id, .. } => key_id,
        }
    }

    fn encrypted(key_id: impl Into<String>, key: &[u8]) -> Result<Self, StorageError> {
        let key_id = key_id.into();
        if key_id.is_empty()
            || key_id.len() > 128
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(StorageError::Encryption("invalid key id".into()));
        }
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| StorageError::Encryption("AES-256-GCM key must be 32 bytes".into()))?;
        Ok(Self::Aes256Gcm {
            key_id,
            cipher: Box::new(cipher),
        })
    }

    fn aad(scope: &Scope, table: &str, record_id: &Id, field: &str) -> Vec<u8> {
        serde_json::to_vec(&(
            "opencoding",
            1_u8,
            &scope.organization_id.0,
            &scope.team_id.0,
            table,
            &record_id.0,
            field,
        ))
        .expect("AAD tuple serialization is infallible")
    }

    fn seal(
        &self,
        scope: &Scope,
        table: &str,
        record_id: &Id,
        field: &str,
        value: &[u8],
    ) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::PlaintextDevelopment => Ok(value.to_vec()),
            Self::Aes256Gcm { key_id, cipher } => {
                let mut nonce = [0_u8; 12];
                getrandom::fill(&mut nonce).map_err(|_| {
                    StorageError::Encryption("secure nonce generation failed".into())
                })?;
                let aad = Self::aad(scope, table, record_id, field);
                let ciphertext = cipher
                    .encrypt(
                        (&nonce).into(),
                        Payload {
                            msg: value,
                            aad: &aad,
                        },
                    )
                    .map_err(|_| StorageError::Encryption("AES-GCM encryption failed".into()))?;
                Ok(format!(
                    "enc:v1:{key_id}:{}:{}",
                    URL_SAFE_NO_PAD.encode(nonce),
                    URL_SAFE_NO_PAD.encode(ciphertext)
                )
                .into_bytes())
            }
        }
    }

    fn open(
        &self,
        scope: &Scope,
        table: &str,
        record_id: &Id,
        field: &str,
        value: &[u8],
    ) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::PlaintextDevelopment => Ok(value.to_vec()),
            Self::Aes256Gcm { key_id, cipher } => {
                let encoded = std::str::from_utf8(value)
                    .map_err(|_| StorageError::Encryption("ciphertext is not UTF-8".into()))?;
                let mut parts = encoded.split(':');
                if parts.next() != Some("enc")
                    || parts.next() != Some("v1")
                    || parts.next() != Some(key_id.as_str())
                {
                    return Err(StorageError::Encryption(
                        "plaintext, unknown key, or malformed ciphertext encountered".into(),
                    ));
                }
                let nonce = URL_SAFE_NO_PAD
                    .decode(parts.next().unwrap_or_default())
                    .map_err(|_| StorageError::Encryption("invalid nonce".into()))?;
                let ciphertext = URL_SAFE_NO_PAD
                    .decode(parts.next().unwrap_or_default())
                    .map_err(|_| StorageError::Encryption("invalid ciphertext".into()))?;
                if parts.next().is_some() || nonce.len() != 12 {
                    return Err(StorageError::Encryption(
                        "malformed ciphertext envelope".into(),
                    ));
                }
                let aad = Self::aad(scope, table, record_id, field);
                cipher
                    .decrypt(
                        nonce.as_slice().into(),
                        Payload {
                            msg: &ciphertext,
                            aad: &aad,
                        },
                    )
                    .map_err(|_| {
                        StorageError::Encryption("ciphertext authentication failed".into())
                    })
            }
        }
    }

    fn seal_text(
        &self,
        scope: &Scope,
        table: &str,
        id: &Id,
        field: &str,
        value: &str,
    ) -> Result<String, StorageError> {
        String::from_utf8(self.seal(scope, table, id, field, value.as_bytes())?)
            .map_err(|_| StorageError::Encryption("encrypted value is not UTF-8".into()))
    }

    fn open_text(
        &self,
        scope: &Scope,
        table: &str,
        id: &Id,
        field: &str,
        value: &str,
    ) -> Result<String, StorageError> {
        String::from_utf8(self.open(scope, table, id, field, value.as_bytes())?)
            .map_err(|_| StorageError::Encryption("decrypted value is not UTF-8".into()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolPolicyMetadata {
    pub simulated_policy: Option<PolicyResult>,
    pub central_policy_applied: bool,
    pub team_configuration_sequence: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledSkillContext {
    pub installation: SkillInstallation,
    pub instructions: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthCredential {
    pub scope: Scope,
    pub server_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
    pub client_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMcpOAuth {
    pub scope: Scope,
    pub state: String,
    pub server_id: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptItemSourceKind {
    Message,
    ToolCall,
    Approval,
    Question,
    Artifact,
    Plan,
    ContextCompaction,
    ModelReroute,
    Usage,
    AgentStatus,
    Hook,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptItemReference {
    pub item_id: Id,
    pub session_id: Id,
    pub turn_id: Id,
    pub source_kind: TranscriptItemSourceKind,
    pub source_id: Id,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptItemReferencePage {
    pub references: Vec<TranscriptItemReference>,
    pub total_count: u64,
    pub has_older: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TranscriptPageSources {
    pub messages: Vec<Message>,
    pub tool_calls: Vec<ToolCall>,
    pub approvals: Vec<Approval>,
    pub questions: Vec<QuestionRequest>,
    pub artifacts: Vec<Artifact>,
    pub events: Vec<Event>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnFileChange {
    pub turn_id: Id,
    pub session_id: Id,
    pub path: String,
    pub before_content: Option<Vec<u8>>,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    pub previous_after_sha256: Option<String>,
    pub state: String,
}

#[derive(Clone, Debug)]
pub struct TurnFileChangePlan {
    pub operation_id: Id,
    pub turn_id: Id,
    pub path: String,
    previous_after_sha256: Option<String>,
    was_new: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CentralAuditExportCursor {
    pub source_id: Id,
    pub organization_id: Id,
    pub team_id: Id,
    pub source_sequence: u64,
    pub local_sequence: u64,
    pub chain_head: String,
}

struct LeasedTaskUpdate<'a> {
    status: &'a str,
    checkpoint: Option<&'a serde_json::Value>,
    result: Option<&'a serde_json::Value>,
    model_cost_micros: Option<u64>,
    runner_cost_micros: Option<u64>,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        Self::connect_with_codec(url, SensitiveCodec::PlaintextDevelopment).await
    }

    pub async fn connect_encrypted(
        url: &str,
        key_id: impl Into<String>,
        key: &[u8],
    ) -> Result<Self, StorageError> {
        Self::connect_with_codec(url, SensitiveCodec::encrypted(key_id, key)?).await
    }

    async fn connect_with_codec(
        url: &str,
        sensitive: SensitiveCodec,
    ) -> Result<Self, StorageError> {
        let database_path = sqlite_database_path(url)?;
        if let Some(path) = database_path.as_deref() {
            prepare_private_database_file(path)?;
        }
        let mut options = SqliteConnectOptions::from_str(url)
            .map_err(StorageError::Database)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        if database_path.is_some() {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        if let Some(path) = database_path.as_deref() {
            enforce_private_file(path)?;
        }
        Self::preflight_unmarked_encrypted_database(&pool, &sensitive).await?;
        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            // Dropping a SQLx pool starts an asynchronous close. Wait for that
            // close here so WAL cleanup and any final checkpoint finish before
            // callers inspect or retry the database after a failed migration.
            pool.close().await;
            return Err(StorageError::Migration(error));
        }
        let store = Self { pool, sensitive };
        store.validate_storage_encryption_metadata().await?;
        store.verify_integrity().await?;
        Ok(store)
    }

    async fn preflight_unmarked_encrypted_database(
        pool: &SqlitePool,
        sensitive: &SensitiveCodec,
    ) -> Result<(), StorageError> {
        if !matches!(sensitive, SensitiveCodec::Aes256Gcm { .. }) {
            return Ok(());
        }
        let marker_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='storage_encryption_metadata'",
        )
        .fetch_one(pool)
        .await?;
        if marker_table != 0 {
            let marker_rows: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM storage_encryption_metadata WHERE singleton=1",
            )
            .fetch_one(pool)
            .await?;
            if marker_rows != 0 {
                return Ok(());
            }
        }
        let settings_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daemon_settings'",
        )
        .fetch_one(pool)
        .await?;
        if settings_table != 0
            && let Some(value) = sqlx::query_scalar::<_, String>(
                "SELECT settings_json FROM daemon_settings WHERE singleton=1",
            )
            .fetch_optional(pool)
            .await?
        {
            if !value.starts_with("enc:v1:") {
                return Err(StorageError::Encryption(
                    "legacy plaintext settings require an explicit export and fresh import".into(),
                ));
            }
            sensitive.open_text(
                &local_settings_scope(),
                "daemon_settings",
                &Id("daemon-settings".into()),
                "settings_json",
                &value,
            )?;
        }

        // Version 43 predates the authenticated storage marker, but many
        // tables could already contain encrypted records without requiring a
        // Session or daemon settings row. Authenticate every existing legacy
        // ciphertext before migration 44 can mutate audit rows or commit the
        // new marker. Each static query projects the exact AAD identity used by
        // the corresponding row decoder.
        let encrypted_columns = [
            (
                "sessions",
                "workspace_uri",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,workspace_uri AS ciphertext FROM sessions WHERE workspace_uri IS NOT NULL",
            ),
            (
                "sessions",
                "title",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,title AS ciphertext FROM sessions WHERE title IS NOT NULL",
            ),
            (
                "sessions",
                "model",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,model AS ciphertext FROM sessions WHERE model IS NOT NULL",
            ),
            (
                "messages",
                "content_json",
                "SELECT m.id AS record_id,s.organization_id,s.team_id,s.actor_id,s.goal_id,s.task_id,m.content_json AS ciphertext FROM messages m INNER JOIN sessions s ON s.id=m.session_id WHERE m.content_json IS NOT NULL",
            ),
            (
                "editor_contexts",
                "context_json",
                "SELECT client_instance_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,context_json AS ciphertext FROM editor_contexts WHERE context_json IS NOT NULL",
            ),
            (
                "session_goals",
                "objective",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,objective AS ciphertext FROM session_goals WHERE objective IS NOT NULL",
            ),
            (
                "session_goals",
                "blocked_reason",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,blocked_reason AS ciphertext FROM session_goals WHERE blocked_reason IS NOT NULL",
            ),
            (
                "session_goal_checkpoints",
                "before_json",
                "SELECT 'goal_checkpoint_' || turn_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,before_json AS ciphertext FROM session_goal_checkpoints WHERE before_json IS NOT NULL",
            ),
            (
                "turn_inputs",
                "content_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,content_json AS ciphertext FROM turn_inputs WHERE content_json IS NOT NULL",
            ),
            (
                "turns",
                "checkpoint_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,checkpoint_json AS ciphertext FROM turns WHERE checkpoint_json IS NOT NULL",
            ),
            (
                "attachments",
                "file_name",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,file_name AS ciphertext FROM attachments WHERE file_name IS NOT NULL",
            ),
            (
                "attachments",
                "media_type",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,media_type AS ciphertext FROM attachments WHERE media_type IS NOT NULL",
            ),
            (
                "attachments",
                "content_base64",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,content_base64 AS ciphertext FROM attachments WHERE content_base64 IS NOT NULL",
            ),
            (
                "attachments",
                "sha256",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,sha256 AS ciphertext FROM attachments WHERE sha256 IS NOT NULL",
            ),
            (
                "background_terminals",
                "spec_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,spec_json AS ciphertext FROM background_terminals WHERE spec_json IS NOT NULL",
            ),
            (
                "background_terminals",
                "program",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,program AS ciphertext FROM background_terminals WHERE program IS NOT NULL",
            ),
            (
                "background_terminals",
                "working_directory_uri",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,working_directory_uri AS ciphertext FROM background_terminals WHERE working_directory_uri IS NOT NULL",
            ),
            (
                "background_terminals",
                "output_base64",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,output_base64 AS ciphertext FROM background_terminals WHERE output_base64 IS NOT NULL",
            ),
            (
                "background_terminals",
                "failure",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,failure AS ciphertext FROM background_terminals WHERE failure IS NOT NULL",
            ),
            (
                "durable_tasks",
                "payload_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,payload_json AS ciphertext FROM durable_tasks WHERE payload_json IS NOT NULL",
            ),
            (
                "durable_tasks",
                "checkpoint_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,checkpoint_json AS ciphertext FROM durable_tasks WHERE checkpoint_json IS NOT NULL",
            ),
            (
                "durable_tasks",
                "result_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,result_json AS ciphertext FROM durable_tasks WHERE result_json IS NOT NULL",
            ),
            (
                "durable_tasks",
                "error",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,error AS ciphertext FROM durable_tasks WHERE error IS NOT NULL",
            ),
            (
                "team_knowledge",
                "content",
                "SELECT id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,content AS ciphertext FROM team_knowledge WHERE content IS NOT NULL",
            ),
            (
                "tool_calls",
                "arguments_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,arguments_json AS ciphertext FROM tool_calls WHERE arguments_json IS NOT NULL",
            ),
            (
                "tool_calls",
                "result_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,result_json AS ciphertext FROM tool_calls WHERE result_json IS NOT NULL",
            ),
            (
                "tool_calls",
                "error",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,error AS ciphertext FROM tool_calls WHERE error IS NOT NULL",
            ),
            (
                "question_requests",
                "questions_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,questions_json AS ciphertext FROM question_requests WHERE questions_json IS NOT NULL",
            ),
            (
                "question_requests",
                "answers_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,answers_json AS ciphertext FROM question_requests WHERE answers_json IS NOT NULL",
            ),
            (
                "artifacts",
                "title",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,title AS ciphertext FROM artifacts WHERE title IS NOT NULL",
            ),
            (
                "artifacts",
                "content_json",
                "SELECT id AS record_id,organization_id,team_id,actor_id,goal_id,task_id,content_json AS ciphertext FROM artifacts WHERE content_json IS NOT NULL",
            ),
            (
                "mcp_installations",
                "spec_json",
                "SELECT 'mcp:' || server_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,spec_json AS ciphertext FROM mcp_installations WHERE spec_json IS NOT NULL",
            ),
            (
                "mcp_http_installations",
                "spec_json",
                "SELECT 'mcp-http:' || server_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,spec_json AS ciphertext FROM mcp_http_installations WHERE spec_json IS NOT NULL",
            ),
            (
                "hook_installations",
                "spec_json",
                "SELECT 'hook:' || hook_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,spec_json AS ciphertext FROM hook_installations WHERE spec_json IS NOT NULL",
            ),
            (
                "skill_installations",
                "spec_json",
                "SELECT 'skill:' || skill_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,spec_json AS ciphertext FROM skill_installations WHERE spec_json IS NOT NULL",
            ),
            (
                "skill_installations",
                "instructions",
                "SELECT 'skill:' || skill_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,instructions AS ciphertext FROM skill_installations WHERE instructions IS NOT NULL",
            ),
            (
                "marketplace_installations",
                "source_json",
                "SELECT 'marketplace:' || marketplace_name AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,source_json AS ciphertext FROM marketplace_installations WHERE source_json IS NOT NULL",
            ),
            (
                "plugin_installations",
                "bundle_json",
                "SELECT 'plugin:' || plugin_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,bundle_json AS ciphertext FROM plugin_installations WHERE bundle_json IS NOT NULL",
            ),
            (
                "mcp_oauth_credentials",
                "credential_json",
                "SELECT 'mcp-oauth:' || server_id AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,credential_json AS ciphertext FROM mcp_oauth_credentials WHERE credential_json IS NOT NULL",
            ),
            (
                "mcp_oauth_pending",
                "pending_json",
                "SELECT 'mcp-oauth-state:' || state AS record_id,organization_id,team_id,actor_id,NULL AS goal_id,NULL AS task_id,pending_json AS ciphertext FROM mcp_oauth_pending WHERE pending_json IS NOT NULL",
            ),
        ];
        for (table, column, query) in encrypted_columns {
            let table_exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
            )
            .bind(table)
            .fetch_one(pool)
            .await?;
            if table_exists == 0 {
                continue;
            }
            for row in sqlx::query(query).fetch_all(pool).await? {
                let ciphertext: String = row.try_get("ciphertext")?;
                if !ciphertext.starts_with("enc:v1:") {
                    return Err(StorageError::Encryption(format!(
                        "legacy plaintext {table}.{column} requires an explicit export and fresh import"
                    )));
                }
                let scope = Scope {
                    organization_id: Id(row.try_get("organization_id")?),
                    team_id: Id(row.try_get("team_id")?),
                    actor_id: Id(row.try_get("actor_id")?),
                    goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
                    task_id: row.try_get::<Option<String>, _>("task_id")?.map(Id),
                };
                sensitive.open_text(
                    &scope,
                    table,
                    &Id(row.try_get("record_id")?),
                    column,
                    &ciphertext,
                )?;
            }
        }
        Ok(())
    }

    async fn validate_storage_encryption_metadata(&self) -> Result<(), StorageError> {
        let marker_scope = Scope {
            organization_id: Id("storage".into()),
            team_id: Id("storage".into()),
            actor_id: Id("storage".into()),
            goal_id: None,
            task_id: None,
        };
        let marker_id = Id("storage-format-v1".into());
        let expected_key_id = self.sensitive.key_id();
        let existing = sqlx::query(
            "SELECT key_id, verification FROM storage_encryption_metadata WHERE singleton=1",
        )
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let stored_key_id: String = row.try_get("key_id")?;
            if stored_key_id != expected_key_id {
                return Err(StorageError::Encryption(format!(
                    "database requires storage key {stored_key_id}, not {expected_key_id}"
                )));
            }
            let verification: String = row.try_get("verification")?;
            let plaintext = self.sensitive.open_text(
                &marker_scope,
                "storage_encryption_metadata",
                &marker_id,
                "verification",
                &verification,
            )?;
            if plaintext != "opencoding-storage-v1" {
                return Err(StorageError::Encryption(
                    "storage encryption verification failed".into(),
                ));
            }
            return Ok(());
        }
        let verification = self.sensitive.seal_text(
            &marker_scope,
            "storage_encryption_metadata",
            &marker_id,
            "verification",
            "opencoding-storage-v1",
        )?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DROP TRIGGER audit_events_no_update")
            .execute(&mut *transaction)
            .await?;
        let mut after_sequence = 0_i64;
        loop {
            let rows = sqlx::query(
                "SELECT sequence,id,organization_id,team_id,actor_id,goal_id,task_id,payload_json \
                 FROM audit_events WHERE sequence>? ORDER BY sequence LIMIT 128",
            )
            .bind(after_sequence)
            .fetch_all(&mut *transaction)
            .await?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                after_sequence = row.try_get("sequence")?;
                let payload: String = row.try_get("payload_json")?;
                let id = Id(row.try_get("id")?);
                let scope = Scope {
                    organization_id: Id(row.try_get("organization_id")?),
                    team_id: Id(row.try_get("team_id")?),
                    actor_id: Id(row.try_get("actor_id")?),
                    goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
                    task_id: row.try_get::<Option<String>, _>("task_id")?.map(Id),
                };
                if payload.starts_with("enc:v1:") {
                    self.sensitive.open_text(
                        &scope,
                        "audit_events",
                        &id,
                        "payload_json",
                        &payload,
                    )?;
                    continue;
                }
                serde_json::from_str::<serde_json::Value>(&payload).map_err(|_| {
                    StorageError::Encryption(
                        "unmarked database contains a malformed audit payload".into(),
                    )
                })?;
                let sealed = self.sensitive.seal_text(
                    &scope,
                    "audit_events",
                    &id,
                    "payload_json",
                    &payload,
                )?;
                sqlx::query("UPDATE audit_events SET payload_json=? WHERE id=?")
                    .bind(sealed)
                    .bind(&id.0)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        sqlx::query(
            "INSERT INTO storage_encryption_metadata(singleton,key_id,verification) VALUES(1,?,?)",
        )
        .bind(expected_key_id)
        .bind(verification)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "CREATE TRIGGER audit_events_no_update \
             BEFORE UPDATE ON audit_events BEGIN \
             SELECT RAISE(ABORT, 'audit events are append-only'); END",
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn inspect_storage_format(url: &str) -> Result<StorageFormat, StorageError> {
        Self::inspect_storage_format_with_mode(url, false).await
    }

    pub async fn inspect_storage_format_without_side_effects(
        url: &str,
    ) -> Result<StorageFormat, StorageError> {
        Self::inspect_storage_format_with_mode(url, true).await
    }

    async fn inspect_storage_format_with_mode(
        url: &str,
        immutable: bool,
    ) -> Result<StorageFormat, StorageError> {
        let Some(path) = sqlite_database_path(url)? else {
            return Ok(StorageFormat::Empty);
        };
        if !path.is_file() || std::fs::metadata(&path)?.len() == 0 {
            return Ok(StorageFormat::Empty);
        }
        let options = SqliteConnectOptions::from_str(url)
            .map_err(StorageError::Database)?
            .read_only(true)
            .immutable(immutable)
            .create_if_missing(false)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let has_marker: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='storage_encryption_metadata'",
        )
        .fetch_one(&pool)
        .await?;
        if has_marker == 1 {
            let key_id = sqlx::query_scalar::<_, String>(
                "SELECT key_id FROM storage_encryption_metadata WHERE singleton=1",
            )
            .fetch_optional(&pool)
            .await?;
            pool.close().await;
            return Ok(
                key_id.map_or(StorageFormat::Unmarked, |key_id| StorageFormat::Marked {
                    key_id,
                }),
            );
        }
        let user_tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_one(&pool)
        .await?;
        pool.close().await;
        if user_tables == 0 {
            Ok(StorageFormat::Empty)
        } else {
            Ok(StorageFormat::Unmarked)
        }
    }

    pub async fn in_memory() -> Result<Self, StorageError> {
        Self::connect("sqlite::memory:").await
    }

    pub async fn close(self) {
        // A read-only SQLite connection may recover a leftover WAL into the
        // database file. Checkpoint graceful shutdowns explicitly so later
        // immutable format inspection can remain byte-for-byte side-effect
        // free when a managed key is missing.
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await;
        self.pool.close().await;
    }

    pub async fn get_or_create_device_id(&self) -> Result<Id, StorageError> {
        let candidate = Id::new("device");
        sqlx::query("INSERT INTO local_identity(singleton,device_id,created_at) VALUES(1,?,?) ON CONFLICT(singleton) DO NOTHING")
            .bind(&candidate.0).bind(Utc::now()).execute(&self.pool).await?;
        let value = sqlx::query_scalar::<_, String>(
            "SELECT device_id FROM local_identity WHERE singleton=1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Id(value))
    }

    pub async fn verify_integrity(&self) -> Result<(), StorageError> {
        let results = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_all(&self.pool)
            .await?;
        if results.len() == 1 && results[0] == "ok" {
            Ok(())
        } else {
            Err(StorageError::InvalidData(format!(
                "SQLite integrity check failed: {}",
                results.join("; ")
            )))
        }
    }

    pub async fn backup_database(&self, destination: &Path) -> Result<(), StorageError> {
        if std::fs::symlink_metadata(destination).is_ok() {
            return Err(StorageError::InvalidState(
                "backup destination already exists".into(),
            ));
        }
        self.verify_integrity().await?;
        let destination_text = destination
            .to_str()
            .ok_or_else(|| StorageError::InvalidData("backup path is not UTF-8".into()))?;
        if let Err(error) = sqlx::query("VACUUM INTO ?")
            .bind(destination_text)
            .execute(&self.pool)
            .await
        {
            let _ = tokio::fs::remove_file(destination).await;
            return Err(StorageError::Database(error));
        }
        enforce_private_file(destination)?;
        if let Err(error) = Self::verify_database_file(destination).await {
            let _ = tokio::fs::remove_file(destination).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn restore_database(backup: &Path, destination: &Path) -> Result<(), StorageError> {
        if backup == destination || !backup.is_file() {
            return Err(StorageError::InvalidData(
                "restore requires distinct existing backup and destination paths".into(),
            ));
        }
        Self::verify_database_file(backup).await?;
        for suffix in ["-wal", "-shm"] {
            if database_sidecar(destination, suffix).exists() {
                return Err(StorageError::InvalidState(
                    "destination database appears open; stop the daemon before restore".into(),
                ));
            }
        }
        let parent = destination.parent().ok_or_else(|| {
            StorageError::InvalidData("restore destination has no parent directory".into())
        })?;
        tokio::fs::create_dir_all(parent).await?;
        let nonce = Id::new("restore").0;
        let staged = parent.join(format!(".opencoding-restore-{nonce}.sqlite"));
        let previous = parent.join(format!(".opencoding-before-restore-{nonce}.sqlite"));
        tokio::fs::copy(backup, &staged).await?;
        enforce_private_file(&staged)?;
        if let Err(error) = Self::verify_database_file(&staged).await {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(error);
        }
        let had_previous = destination.exists();
        if had_previous {
            tokio::fs::rename(destination, &previous).await?;
        }
        if let Err(error) = tokio::fs::rename(&staged, destination).await {
            if had_previous {
                let _ = tokio::fs::rename(&previous, destination).await;
            }
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(StorageError::Io(error));
        }
        if had_previous {
            tokio::fs::remove_file(previous).await?;
        }
        enforce_private_file(destination)?;
        Ok(())
    }

    async fn verify_database_file(path: &Path) -> Result<(), StorageError> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(StorageError::InvalidData(
                "SQLite database path must not be a symbolic link".into(),
            ));
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let results = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_all(&pool)
            .await?;
        pool.close().await;
        if results.len() == 1 && results[0] == "ok" {
            Ok(())
        } else {
            Err(StorageError::InvalidData(format!(
                "backup integrity check failed: {}",
                results.join("; ")
            )))
        }
    }

    pub async fn get_settings(&self) -> Result<DaemonSettings, StorageError> {
        let encoded = sqlx::query_scalar::<_, String>(
            "SELECT settings_json FROM daemon_settings WHERE singleton=1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let scope = local_settings_scope();
        let id = Id("daemon-settings".into());
        encoded
            .map(|value| {
                self.sensitive
                    .open_text(&scope, "daemon_settings", &id, "settings_json", &value)
            })
            .transpose()?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map(|value| value.unwrap_or_default())
            .map_err(|error| StorageError::InvalidData(error.to_string()))
    }

    pub async fn put_settings(
        &self,
        settings: &DaemonSettings,
    ) -> Result<DaemonSettings, StorageError> {
        validate_settings(settings)?;
        let encoded = serde_json::to_string(settings)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            &local_settings_scope(),
            "daemon_settings",
            &Id("daemon-settings".into()),
            "settings_json",
            &encoded,
        )?;
        sqlx::query("INSERT INTO daemon_settings(singleton,settings_json,updated_at) VALUES(1,?,?) ON CONFLICT(singleton) DO UPDATE SET settings_json=excluded.settings_json,updated_at=excluded.updated_at")
            .bind(encoded).bind(Utc::now()).execute(&self.pool).await?;
        Ok(settings.clone())
    }

    pub async fn create_session(&self, input: CreateSession) -> Result<Session, StorageError> {
        let now = Utc::now();
        let session = Session {
            id: Id::new("ses"),
            scope: input.scope,
            workspace_uri: input.workspace_uri,
            title: input.title,
            model: input.model,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
        };
        let workspace_uri = self.sensitive.seal_text(
            &session.scope,
            "sessions",
            &session.id,
            "workspace_uri",
            &session.workspace_uri,
        )?;
        let title = self.sensitive.seal_text(
            &session.scope,
            "sessions",
            &session.id,
            "title",
            &session.title,
        )?;
        let model = self.sensitive.seal_text(
            &session.scope,
            "sessions",
            &session.id,
            "model",
            &session.model,
        )?;
        sqlx::query("INSERT INTO sessions (id,organization_id,team_id,actor_id,goal_id,task_id,workspace_uri,title,model,status,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,'active',?,?)")
            .bind(&session.id.0).bind(&session.scope.organization_id.0).bind(&session.scope.team_id.0).bind(&session.scope.actor_id.0)
            .bind(session.scope.goal_id.as_ref().map(|v| &v.0)).bind(session.scope.task_id.as_ref().map(|v| &v.0))
            .bind(workspace_uri).bind(title).bind(model).bind(now).bind(now).execute(&self.pool).await?;
        Ok(session)
    }

    pub async fn get_session(&self, id: &Id) -> Result<Session, StorageError> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ? AND status != 'deleted'")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        row_to_session(&row, &self.sensitive)
    }

    pub async fn list_sessions(&self, scope: &Scope) -> Result<Vec<Session>, StorageError> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE organization_id = ? AND team_id = ? AND actor_id = ? AND status != 'deleted' ORDER BY updated_at DESC")
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&scope.actor_id.0)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| row_to_session(row, &self.sensitive))
            .collect()
    }

    pub async fn update_session_title(
        &self,
        scope: &Scope,
        session_id: &Id,
        title: &str,
    ) -> Result<Session, StorageError> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 64 || title.chars().any(char::is_control) {
            return Err(StorageError::InvalidData(
                "session title must be 1-64 printable characters".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let title = self
            .sensitive
            .seal_text(scope, "sessions", session_id, "title", title)?;
        sqlx::query("UPDATE sessions SET title=?,updated_at=? WHERE id=?")
            .bind(title)
            .bind(Utc::now())
            .bind(&session_id.0)
            .execute(&self.pool)
            .await?;
        self.get_session(session_id).await
    }

    pub async fn update_session_model(
        &self,
        scope: &Scope,
        session_id: &Id,
        model: &str,
    ) -> Result<Session, StorageError> {
        let model = model.trim();
        if model.is_empty() || model.chars().count() > 200 || model.chars().any(char::is_control) {
            return Err(StorageError::InvalidData(
                "session model must be 1-200 printable characters".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let model = self
            .sensitive
            .seal_text(scope, "sessions", session_id, "model", model)?;
        sqlx::query("UPDATE sessions SET model=?,updated_at=? WHERE id=?")
            .bind(model)
            .bind(Utc::now())
            .bind(&session_id.0)
            .execute(&self.pool)
            .await?;
        self.get_session(session_id).await
    }

    pub async fn get_session_preferences(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<SessionPreferences, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row = sqlx::query(
            "SELECT permission_mode,assistant_alias,source,locked_reason,updated_at \
             FROM session_preferences WHERE session_id=?",
        )
        .bind(&session_id.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(SessionPreferences {
                session_id: session_id.clone(),
                permission_mode: PermissionMode::Manual,
                assistant_alias: "Opencoding".into(),
                source: "default".into(),
                locked_reason: None,
                updated_at: session.created_at,
            });
        };
        let permission_mode = match row.try_get::<String, _>("permission_mode")?.as_str() {
            "manual" => PermissionMode::Manual,
            "accept_edits" => PermissionMode::AcceptEdits,
            "workspace" => PermissionMode::Workspace,
            "plan" => PermissionMode::Plan,
            value => {
                return Err(StorageError::InvalidData(format!(
                    "invalid permission mode {value}"
                )));
            }
        };
        Ok(SessionPreferences {
            session_id: session_id.clone(),
            permission_mode,
            assistant_alias: row.try_get("assistant_alias")?,
            source: row.try_get("source")?,
            locked_reason: row.try_get("locked_reason")?,
            updated_at: row.try_get("updated_at")?,
        })
    }

    pub async fn update_session_preferences(
        &self,
        session_id: &Id,
        input: UpdateSessionPreferences,
    ) -> Result<SessionPreferences, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        if input.permission_mode.is_none() && input.assistant_alias.is_none() {
            return Err(StorageError::InvalidData(
                "at least one Session preference must be provided".into(),
            ));
        }
        let current = self
            .get_session_preferences(&input.scope, session_id)
            .await?;
        let permission_mode = match input.permission_mode.unwrap_or(current.permission_mode) {
            PermissionMode::Manual => "manual",
            PermissionMode::AcceptEdits => "accept_edits",
            PermissionMode::Workspace => "workspace",
            PermissionMode::Plan => "plan",
        };
        let assistant_alias = input
            .assistant_alias
            .as_deref()
            .unwrap_or(&current.assistant_alias)
            .trim();
        if assistant_alias.is_empty()
            || assistant_alias.chars().count() > 40
            || assistant_alias.chars().any(char::is_control)
        {
            return Err(StorageError::InvalidData(
                "assistant alias must contain 1 to 40 visible characters".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO session_preferences \
             (session_id,organization_id,team_id,permission_mode,assistant_alias,source,locked_reason,updated_at) \
             VALUES (?,?,?,?,?, 'session', NULL, ?) \
             ON CONFLICT(session_id) DO UPDATE SET \
             permission_mode=excluded.permission_mode,assistant_alias=excluded.assistant_alias,source=excluded.source,\
             locked_reason=NULL,updated_at=excluded.updated_at",
        )
        .bind(&session_id.0)
        .bind(&input.scope.organization_id.0)
        .bind(&input.scope.team_id.0)
        .bind(permission_mode)
        .bind(assistant_alias)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        self.get_session_preferences(&input.scope, session_id).await
    }

    pub async fn get_session_goal(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Option<SessionGoal>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row = sqlx::query("SELECT * FROM session_goals WHERE session_id=?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref()
            .map(|row| row_to_session_goal(row, &self.sensitive))
            .transpose()
    }

    /// Opens the Session Goal participant before a Turn mutates it. Repeated
    /// mutations in one Turn preserve the first before-image and replace only
    /// the expected after-image when finalized.
    pub async fn begin_session_goal_checkpoint(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        before: Option<&SessionGoal>,
    ) -> Result<(), StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let turn = self.get_turn(scope, turn_id).await?;
        if turn.session_id != *session_id || is_terminal(&turn.status) {
            return Err(StorageError::InvalidState(
                "Session Goal checkpoint requires the active owning Turn".into(),
            ));
        }
        let checkpoint_id = Id(format!("goal_checkpoint_{}", turn_id.0));
        let before_json = before
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?
            .map(|value| {
                self.sensitive.seal_text(
                    scope,
                    "session_goal_checkpoints",
                    &checkpoint_id,
                    "before_json",
                    &value,
                )
            })
            .transpose()?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        let existing = sqlx::query("SELECT state FROM session_goal_checkpoints WHERE turn_id=?")
            .bind(&turn_id.0)
            .fetch_optional(&mut *transaction)
            .await?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO session_goal_checkpoints
                     (turn_id,session_id,organization_id,team_id,actor_id,before_json,after_sha256,state,created_at,updated_at,undone_at)
                     VALUES (?,?,?,?,?,?,NULL,'pending',?,?,NULL)",
                )
                .bind(&turn_id.0)
                .bind(&session_id.0)
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0)
                .bind(&scope.actor_id.0)
                .bind(before_json)
                .bind(now)
                .bind(now)
                .execute(&mut *transaction)
                .await?;
            }
            Some(row) if row.try_get::<String, _>("state")? == "active" => {
                sqlx::query(
                    "UPDATE session_goal_checkpoints
                     SET after_sha256=NULL,state='pending',updated_at=?
                     WHERE turn_id=? AND state='active'",
                )
                .bind(now)
                .bind(&turn_id.0)
                .execute(&mut *transaction)
                .await?;
            }
            Some(row) => {
                let state: String = row.try_get("state")?;
                return Err(StorageError::InvalidState(format!(
                    "Session Goal checkpoint is {state}"
                )));
            }
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn finish_session_goal_checkpoint(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        after: Option<&SessionGoal>,
    ) -> Result<(), StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let after_sha256 = session_goal_conversation_sha256(after)?;
        let changed = sqlx::query(
            "UPDATE session_goal_checkpoints
             SET after_sha256=?,state='active',updated_at=?
             WHERE turn_id=? AND session_id=? AND state='pending'",
        )
        .bind(after_sha256)
        .bind(Utc::now())
        .bind(&turn_id.0)
        .bind(&session_id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Session Goal checkpoint is no longer pending".into(),
            ));
        }
        Ok(())
    }

    pub async fn refresh_session_goal_checkpoint(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        after: Option<&SessionGoal>,
    ) -> Result<bool, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let after_sha256 = session_goal_conversation_sha256(after)?;
        let changed = sqlx::query(
            "UPDATE session_goal_checkpoints
             SET after_sha256=?,state='active',updated_at=?
             WHERE turn_id=? AND session_id=? AND state IN ('pending','active')",
        )
        .bind(after_sha256)
        .bind(Utc::now())
        .bind(&turn_id.0)
        .bind(&session_id.0)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    /// Restores only conversation-time Goal controls. Usage remains in the
    /// audited Turn aggregate and a concurrent Goal mutation fails closed.
    pub async fn session_goal_checkpoint_is_restorable(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
    ) -> Result<bool, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row = sqlx::query(
            "SELECT organization_id,team_id,actor_id,after_sha256,state
             FROM session_goal_checkpoints WHERE turn_id=? AND session_id=?",
        )
        .bind(&turn_id.0)
        .bind(&session_id.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        if row.try_get::<String, _>("organization_id")? != scope.organization_id.0
            || row.try_get::<String, _>("team_id")? != scope.team_id.0
            || row.try_get::<String, _>("actor_id")? != scope.actor_id.0
        {
            return Err(StorageError::ScopeMismatch);
        }
        let state: String = row.try_get("state")?;
        if state == "undone" {
            return Ok(false);
        }
        if state == "pending" {
            return Ok(true);
        }
        if state != "active" {
            return Err(StorageError::InvalidData(
                "invalid Session Goal checkpoint state".into(),
            ));
        }
        let expected: Option<String> = row.try_get("after_sha256")?;
        let current = self.get_session_goal(scope, session_id).await?;
        let current_sha256 = session_goal_conversation_sha256(current.as_ref())?;
        if expected.as_deref() != Some(current_sha256.as_str()) {
            return Err(StorageError::InvalidState(
                "Session Goal changed after the target Turn".into(),
            ));
        }
        Ok(true)
    }

    pub async fn restore_session_goal_checkpoint(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
    ) -> Result<bool, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row =
            sqlx::query("SELECT * FROM session_goal_checkpoints WHERE turn_id=? AND session_id=?")
                .bind(&turn_id.0)
                .bind(&session_id.0)
                .fetch_optional(&self.pool)
                .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        if row.try_get::<String, _>("organization_id")? != scope.organization_id.0
            || row.try_get::<String, _>("team_id")? != scope.team_id.0
            || row.try_get::<String, _>("actor_id")? != scope.actor_id.0
        {
            return Err(StorageError::ScopeMismatch);
        }
        let checkpoint_state: String = row.try_get("state")?;
        if checkpoint_state == "undone" {
            return Ok(false);
        }
        let current = self.get_session_goal(scope, session_id).await?;
        if checkpoint_state == "active" {
            let expected: Option<String> = row.try_get("after_sha256")?;
            let current_sha256 = session_goal_conversation_sha256(current.as_ref())?;
            if expected.as_deref() != Some(current_sha256.as_str()) {
                return Err(StorageError::InvalidState(
                    "Session Goal changed after the target Turn".into(),
                ));
            }
        } else if checkpoint_state != "pending" {
            return Err(StorageError::InvalidData(
                "invalid Session Goal checkpoint state".into(),
            ));
        }
        let checkpoint_id = Id(format!("goal_checkpoint_{}", turn_id.0));
        let before = row
            .try_get::<Option<String>, _>("before_json")?
            .map(|sealed| {
                self.sensitive.open_text(
                    scope,
                    "session_goal_checkpoints",
                    &checkpoint_id,
                    "before_json",
                    &sealed,
                )
            })
            .transpose()?
            .map(|value| {
                serde_json::from_str::<SessionGoal>(&value)
                    .map_err(|error| StorageError::InvalidData(error.to_string()))
            })
            .transpose()?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM session_goals WHERE session_id=?")
            .bind(&session_id.0)
            .execute(&mut *transaction)
            .await?;
        if let Some(before) = before {
            if before.session_id != *session_id
                || before.scope.organization_id != scope.organization_id
                || before.scope.team_id != scope.team_id
                || before.scope.actor_id != scope.actor_id
            {
                return Err(StorageError::InvalidData(
                    "Session Goal checkpoint before-image has the wrong scope".into(),
                ));
            }
            let objective = self.sensitive.seal_text(
                scope,
                "session_goals",
                &before.id,
                "objective",
                &before.objective,
            )?;
            let blocked_reason = before
                .blocked_reason
                .as_deref()
                .map(|reason| {
                    self.sensitive.seal_text(
                        scope,
                        "session_goals",
                        &before.id,
                        "blocked_reason",
                        reason,
                    )
                })
                .transpose()?;
            let input_tokens = current
                .as_ref()
                .map_or(before.input_tokens, |goal| goal.input_tokens);
            let output_tokens = current
                .as_ref()
                .map_or(before.output_tokens, |goal| goal.output_tokens);
            let continuation_count = current
                .as_ref()
                .map_or(before.continuation_count, |goal| goal.continuation_count);
            let last_turn_id = current
                .as_ref()
                .and_then(|goal| goal.last_turn_id.as_ref())
                .or(before.last_turn_id.as_ref());
            let revision = current
                .as_ref()
                .map_or(before.revision, |goal| goal.revision)
                .saturating_add(1);
            sqlx::query(
                "INSERT INTO session_goals
                 (id,session_id,organization_id,team_id,actor_id,objective,status,auto_continue,
                  token_budget,input_tokens,output_tokens,continuation_count,last_turn_id,
                  blocked_reason,revision,created_at,updated_at,completed_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(&before.id.0)
            .bind(&session_id.0)
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&scope.actor_id.0)
            .bind(objective)
            .bind(session_goal_status_str(&before.status))
            .bind(before.auto_continue)
            .bind(
                before
                    .token_budget
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| {
                        StorageError::InvalidData("Goal token budget is too large".into())
                    })?,
            )
            .bind(
                i64::try_from(input_tokens).map_err(|_| {
                    StorageError::InvalidData("Goal input usage is too large".into())
                })?,
            )
            .bind(
                i64::try_from(output_tokens).map_err(|_| {
                    StorageError::InvalidData("Goal output usage is too large".into())
                })?,
            )
            .bind(i64::from(continuation_count))
            .bind(last_turn_id.map(|id| id.0.as_str()))
            .bind(blocked_reason)
            .bind(
                i64::try_from(revision)
                    .map_err(|_| StorageError::InvalidData("Goal revision is too large".into()))?,
            )
            .bind(before.created_at)
            .bind(now)
            .bind(before.completed_at)
            .execute(&mut *transaction)
            .await?;
        }
        let changed = sqlx::query(
            "UPDATE session_goal_checkpoints
             SET state='undone',updated_at=?,undone_at=?
             WHERE turn_id=? AND state IN ('pending','active')",
        )
        .bind(now)
        .bind(now)
        .bind(&turn_id.0)
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Session Goal checkpoint changed concurrently".into(),
            ));
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn set_session_goal(
        &self,
        session_id: &Id,
        input: SetSessionGoal,
    ) -> Result<SessionGoal, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let objective = validate_session_goal_text("objective", &input.objective, 4_000)?;
        let token_budget = input
            .token_budget
            .map(|value| {
                if value == 0 {
                    return Err(StorageError::InvalidData(
                        "session goal token budget must be positive".into(),
                    ));
                }
                i64::try_from(value)
                    .map_err(|_| StorageError::InvalidData("token budget is too large".into()))
            })
            .transpose()?;
        let id = Id::new("sgoal");
        let sealed_objective =
            self.sensitive
                .seal_text(&input.scope, "session_goals", &id, "objective", objective)?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("DELETE FROM session_goals WHERE session_id=?")
            .bind(&session_id.0)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO session_goals \
             (id,session_id,organization_id,team_id,actor_id,objective,status,auto_continue,\
              token_budget,input_tokens,output_tokens,continuation_count,last_turn_id,\
              blocked_reason,revision,created_at,updated_at,completed_at) \
             VALUES (?,?,?,?,?,?,'active',?,?,0,0,0,NULL,NULL,1,?,?,NULL)",
        )
        .bind(&id.0)
        .bind(&session_id.0)
        .bind(&input.scope.organization_id.0)
        .bind(&input.scope.team_id.0)
        .bind(&input.scope.actor_id.0)
        .bind(sealed_objective)
        .bind(input.auto_continue)
        .bind(token_budget)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get_session_goal(&input.scope, session_id)
            .await?
            .ok_or(StorageError::NotFound)
    }

    pub async fn update_session_goal(
        &self,
        session_id: &Id,
        input: UpdateSessionGoal,
    ) -> Result<SessionGoal, StorageError> {
        let current = self
            .get_session_goal(&input.scope, session_id)
            .await?
            .ok_or(StorageError::NotFound)?;
        if current.revision != input.expected_revision {
            return Err(StorageError::InvalidState(
                "session goal changed concurrently".into(),
            ));
        }
        if input.objective.is_none()
            && input.status.is_none()
            && input.auto_continue.is_none()
            && input.blocked_reason.is_none()
        {
            return Err(StorageError::InvalidData(
                "session goal update requires a change".into(),
            ));
        }
        let objective = input
            .objective
            .as_deref()
            .map(|value| validate_session_goal_text("objective", value, 4_000))
            .transpose()?
            .unwrap_or(current.objective.as_str());
        let next_status = input.status.unwrap_or_else(|| current.status.clone());
        validate_session_goal_transition(&current.status, &next_status)?;
        let blocked_reason = if next_status == SessionGoalStatus::Blocked {
            let reason = input
                .blocked_reason
                .as_deref()
                .or(current.blocked_reason.as_deref())
                .unwrap_or("The goal needs user input or an external state change.");
            Some(validate_session_goal_text("blocked reason", reason, 1_000)?)
        } else {
            None
        };
        let sealed_objective = self.sensitive.seal_text(
            &input.scope,
            "session_goals",
            &current.id,
            "objective",
            objective,
        )?;
        let sealed_reason = blocked_reason
            .map(|reason| {
                self.sensitive.seal_text(
                    &input.scope,
                    "session_goals",
                    &current.id,
                    "blocked_reason",
                    reason,
                )
            })
            .transpose()?;
        let status = session_goal_status_str(&next_status);
        let completed_at = if next_status == SessionGoalStatus::Completed {
            Some(Utc::now())
        } else {
            current.completed_at
        };
        let updated = sqlx::query(
            "UPDATE session_goals SET objective=?,status=?,auto_continue=?,blocked_reason=?,\
             completed_at=?,updated_at=?,revision=revision+1 \
             WHERE session_id=? AND revision=?",
        )
        .bind(sealed_objective)
        .bind(status)
        .bind(input.auto_continue.unwrap_or(current.auto_continue))
        .bind(sealed_reason)
        .bind(completed_at)
        .bind(Utc::now())
        .bind(&session_id.0)
        .bind(
            i64::try_from(current.revision)
                .map_err(|_| StorageError::InvalidData("goal revision is too large".into()))?,
        )
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "session goal changed concurrently".into(),
            ));
        }
        self.get_session_goal(&input.scope, session_id)
            .await?
            .ok_or(StorageError::NotFound)
    }

    pub async fn clear_session_goal(
        &self,
        scope: &Scope,
        session_id: &Id,
        expected_revision: u64,
    ) -> Result<SessionGoal, StorageError> {
        let current = self
            .get_session_goal(scope, session_id)
            .await?
            .ok_or(StorageError::NotFound)?;
        if current.revision != expected_revision {
            return Err(StorageError::InvalidState(
                "session goal changed concurrently".into(),
            ));
        }
        let deleted = sqlx::query("DELETE FROM session_goals WHERE session_id=? AND revision=?")
            .bind(&session_id.0)
            .bind(
                i64::try_from(expected_revision)
                    .map_err(|_| StorageError::InvalidData("goal revision is too large".into()))?,
            )
            .execute(&self.pool)
            .await?;
        if deleted.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "session goal changed concurrently".into(),
            ));
        }
        Ok(current)
    }

    pub async fn record_session_goal_turn(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<Option<SessionGoal>, StorageError> {
        let Some(current) = self.get_session_goal(scope, session_id).await? else {
            return Ok(None);
        };
        let input_tokens = i64::try_from(input_tokens)
            .map_err(|_| StorageError::InvalidData("input token usage is too large".into()))?;
        let output_tokens = i64::try_from(output_tokens)
            .map_err(|_| StorageError::InvalidData("output token usage is too large".into()))?;
        let would_use = current
            .input_tokens
            .saturating_add(current.output_tokens)
            .saturating_add(input_tokens as u64)
            .saturating_add(output_tokens as u64);
        let exhausted = current
            .token_budget
            .is_some_and(|budget| would_use >= budget)
            && current.status == SessionGoalStatus::Active;
        let blocked_reason = if exhausted {
            Some(self.sensitive.seal_text(
                scope,
                "session_goals",
                &current.id,
                "blocked_reason",
                "The configured token budget has been exhausted.",
            )?)
        } else {
            current
                .blocked_reason
                .as_deref()
                .map(|reason| {
                    self.sensitive.seal_text(
                        scope,
                        "session_goals",
                        &current.id,
                        "blocked_reason",
                        reason,
                    )
                })
                .transpose()?
        };
        sqlx::query(
            "UPDATE session_goals SET input_tokens=input_tokens+?,output_tokens=output_tokens+?,\
             continuation_count=continuation_count+1,last_turn_id=?,\
             status=CASE WHEN ? THEN 'blocked' ELSE status END,blocked_reason=?,\
             updated_at=?,revision=revision+1 WHERE session_id=?",
        )
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(&turn_id.0)
        .bind(exhausted)
        .bind(blocked_reason)
        .bind(Utc::now())
        .bind(&session_id.0)
        .execute(&self.pool)
        .await?;
        self.get_session_goal(scope, session_id).await
    }

    pub async fn record_session_fork(
        &self,
        scope: &Scope,
        session_id: &Id,
        parent_session_id: &Id,
        source_turn_id: Option<&Id>,
    ) -> Result<SessionForkMetadata, StorageError> {
        let session = self.get_session(session_id).await?;
        let parent = self.get_session(parent_session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        ensure_actor_session_scope(&parent, scope)?;
        if session_id == parent_session_id {
            return Err(StorageError::InvalidData(
                "a session cannot be its own parent".into(),
            ));
        }
        if let Some(source_turn_id) = source_turn_id {
            let source_turn = self.get_turn(scope, source_turn_id).await?;
            if source_turn.session_id != *parent_session_id {
                return Err(StorageError::InvalidData(
                    "source turn does not belong to the parent session".into(),
                ));
            }
        }
        let created_at = Utc::now();
        sqlx::query(
            "INSERT INTO session_forks \
             (session_id,parent_session_id,source_turn_id,organization_id,team_id,created_at) \
             VALUES (?,?,?,?,?,?)",
        )
        .bind(&session_id.0)
        .bind(&parent_session_id.0)
        .bind(source_turn_id.map(|id| &id.0))
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(SessionForkMetadata {
            session_id: session_id.clone(),
            parent_session_id: parent_session_id.clone(),
            source_turn_id: source_turn_id.cloned(),
            created_at,
        })
    }

    pub async fn list_session_forks(
        &self,
        scope: &Scope,
    ) -> Result<Vec<SessionForkMetadata>, StorageError> {
        let rows = sqlx::query(
            "SELECT session_id,parent_session_id,source_turn_id,created_at \
             FROM session_forks WHERE organization_id=? AND team_id=? \
             ORDER BY created_at,session_id",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(SessionForkMetadata {
                    session_id: Id(row.try_get("session_id")?),
                    parent_session_id: Id(row.try_get("parent_session_id")?),
                    source_turn_id: row.try_get::<Option<String>, _>("source_turn_id")?.map(Id),
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    pub async fn create_side_conversation(
        &self,
        scope: &Scope,
        source_session_id: &Id,
        session_id: &Id,
        source_turn_id: Option<&Id>,
    ) -> Result<SideConversation, StorageError> {
        let source = self.get_session(source_session_id).await?;
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&source, scope)?;
        ensure_actor_session_scope(&session, scope)?;
        if source_session_id == session_id {
            return Err(StorageError::InvalidData(
                "a side conversation requires a distinct Session".into(),
            ));
        }
        if let Some(source_turn_id) = source_turn_id {
            let turn = self.get_turn(scope, source_turn_id).await?;
            if turn.session_id != *source_session_id {
                return Err(StorageError::InvalidData(
                    "side conversation source Turn is not in the source Session".into(),
                ));
            }
        }
        let fork_parent: Option<String> =
            sqlx::query_scalar("SELECT parent_session_id FROM session_forks WHERE session_id=?")
                .bind(&session_id.0)
                .fetch_optional(&self.pool)
                .await?;
        if fork_parent.as_deref() != Some(source_session_id.0.as_str()) {
            return Err(StorageError::InvalidState(
                "side conversation Session is not a fork of its source".into(),
            ));
        }
        let now = Utc::now();
        let conversation = SideConversation {
            id: Id::new("side"),
            scope: scope.clone(),
            source_session_id: source_session_id.clone(),
            session_id: session_id.clone(),
            source_turn_id: source_turn_id.cloned(),
            status: SideConversationStatus::Active,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(
            "INSERT INTO side_conversations \
             (id,organization_id,team_id,actor_id,goal_id,task_id,source_session_id,session_id,source_turn_id,status,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,?,?,?,'active',?,?)",
        )
        .bind(&conversation.id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(scope.goal_id.as_ref().map(|id| &id.0))
        .bind(scope.task_id.as_ref().map(|id| &id.0))
        .bind(&source_session_id.0)
        .bind(&session_id.0)
        .bind(source_turn_id.map(|id| &id.0))
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(conversation)
    }

    pub async fn list_side_conversations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<SideConversation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM side_conversations \
             WHERE organization_id=? AND team_id=? AND actor_id=? \
             ORDER BY updated_at DESC,id",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_side_conversation).collect()
    }

    pub async fn get_side_conversation(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<SideConversation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM side_conversations \
             WHERE id=? AND organization_id=? AND team_id=? AND actor_id=?",
        )
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        row_to_side_conversation(&row)
    }

    pub async fn update_side_conversation_status(
        &self,
        scope: &Scope,
        id: &Id,
        status: SideConversationStatus,
    ) -> Result<SideConversation, StorageError> {
        let current = self.get_side_conversation(scope, id).await?;
        if current.status == status {
            return Ok(current);
        }
        if current.status != SideConversationStatus::Active
            || status == SideConversationStatus::Active
        {
            return Err(StorageError::InvalidState(
                "side conversation is already terminal".into(),
            ));
        }
        let value = match status {
            SideConversationStatus::Active => unreachable!(),
            SideConversationStatus::Promoted => "promoted",
            SideConversationStatus::Closed => "closed",
        };
        let updated = sqlx::query(
            "UPDATE side_conversations SET status=?,updated_at=? \
             WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status='active'",
        )
        .bind(value)
        .bind(Utc::now())
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "side conversation changed concurrently".into(),
            ));
        }
        self.get_side_conversation(scope, id).await
    }

    pub async fn restore_session(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Session, StorageError> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_session(&row, &self.sensitive)?;
        ensure_actor_session_scope(&current, scope)?;
        match current.status {
            SessionStatus::Active => return Ok(current),
            SessionStatus::Deleted => return Err(StorageError::NotFound),
            SessionStatus::Archived => {}
        }
        let updated = sqlx::query(
            "UPDATE sessions SET status='active',updated_at=? WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status='archived'",
        )
        .bind(Utc::now())
        .bind(&session_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "session changed concurrently".into(),
            ));
        }
        self.get_session(session_id).await
    }

    pub async fn active_session_turn_ids(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<Id>, StorageError> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let session = row_to_session(&row, &self.sensitive)?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status == SessionStatus::Deleted {
            return Ok(Vec::new());
        }
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM turns WHERE session_id=? AND status NOT IN ('completed','failed','cancelled') ORDER BY started_at",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(ids.into_iter().map(Id).collect())
    }

    pub async fn close_session(
        &self,
        scope: &Scope,
        session_id: &Id,
        status: SessionStatus,
    ) -> Result<Session, StorageError> {
        if !matches!(status, SessionStatus::Archived | SessionStatus::Deleted) {
            return Err(StorageError::InvalidState(
                "session may only be archived or deleted".into(),
            ));
        }
        let row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_session(&row, &self.sensitive)?;
        ensure_actor_session_scope(&current, scope)?;
        if current.status == SessionStatus::Deleted {
            return Ok(current);
        }
        if current.status == SessionStatus::Archived && status == SessionStatus::Archived {
            return Ok(current);
        }
        let now = Utc::now();
        let status_value = match status {
            SessionStatus::Archived => "archived",
            SessionStatus::Deleted => "deleted",
            SessionStatus::Active => unreachable!(),
        };
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE turns SET status='cancelled',error_code='session_closed',updated_at=?,completed_at=? WHERE session_id=? AND status NOT IN ('completed','failed','cancelled')")
            .bind(now).bind(now).bind(&session_id.0).execute(&mut *transaction).await?;
        sqlx::query("UPDATE question_requests SET status='cancelled',revision=revision+1 WHERE session_id=? AND status='pending'")
            .bind(&session_id.0).execute(&mut *transaction).await?;
        sqlx::query("UPDATE turn_inputs SET status='cancelled',updated_at=?,cancelled_at=?,revision=revision+1 WHERE session_id=? AND status IN ('pending','processing')")
            .bind(now).bind(now).bind(&session_id.0).execute(&mut *transaction).await?;
        let updated = sqlx::query("UPDATE sessions SET status=?,updated_at=? WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status!='deleted'")
            .bind(status_value).bind(now).bind(&session_id.0).bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&scope.actor_id.0).execute(&mut *transaction).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "session changed concurrently".into(),
            ));
        }
        transaction.commit().await?;
        let row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(&session_id.0)
            .fetch_one(&self.pool)
            .await?;
        row_to_session(&row, &self.sensitive)
    }

    pub async fn append_message(
        &self,
        scope: &Scope,
        session_id: &Id,
        role: &str,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        self.append_turn_message(scope, session_id, &Id::new("turn"), role, content)
            .await
    }

    pub async fn append_turn_message(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        role: &str,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        self.append_turn_message_with_id(scope, session_id, turn_id, Id::new("msg"), role, content)
            .await
    }

    pub async fn append_turn_message_with_id(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        message_id: Id,
        role: &str,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        let session = self.get_session(session_id).await?;
        if session.scope.organization_id != scope.organization_id
            || session.scope.team_id != scope.team_id
        {
            return Err(StorageError::ScopeMismatch);
        }
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let message = Message {
            id: message_id,
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            role: role.into(),
            content,
            created_at: Utc::now(),
        };
        let content_json = serde_json::to_string(&message.content)
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let content_json = self.sensitive.seal_text(
            scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        sqlx::query("INSERT INTO messages (id,session_id,turn_id,role,content_json,created_at) VALUES (?,?,?,?,?,?)")
            .bind(&message.id.0).bind(&message.session_id.0).bind(&message.turn_id.0).bind(&message.role).bind(content_json).bind(message.created_at).execute(&self.pool).await?;
        sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind(message.created_at)
            .bind(&session_id.0)
            .execute(&self.pool)
            .await?;
        Ok(message)
    }

    /// Appends a durable context-compaction marker only if the source message
    /// set is unchanged. Messages are immutable outside bounded streaming
    /// finalization, and compaction is permitted only after a terminal Turn.
    pub async fn append_context_compaction(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        expected_message_count: u64,
        expected_latest_message_id: Option<&Id>,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        self.append_context_state(
            scope,
            session_id,
            turn_id,
            expected_message_count,
            expected_latest_message_id,
            "context_compaction_state",
            content,
        )
        .await
    }

    /// Records a logical conversation-time Undo without deleting the audited
    /// transcript. Consumers project the omitted message identities out of
    /// future model context while transcript and audit history remain intact.
    pub async fn append_conversation_undo(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        expected_message_count: u64,
        expected_latest_message_id: Option<&Id>,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        self.append_context_state(
            scope,
            session_id,
            turn_id,
            expected_message_count,
            expected_latest_message_id,
            "conversation_undo_state",
            content,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn append_context_state(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        expected_message_count: u64,
        expected_latest_message_id: Option<&Id>,
        role: &str,
        content: serde_json::Value,
    ) -> Result<Message, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let turn = self.get_turn(scope, turn_id).await?;
        if turn.session_id != *session_id || !is_terminal(&turn.status) {
            return Err(StorageError::InvalidState(
                "context compaction requires a terminal Turn in the same Session".into(),
            ));
        }
        let message = Message {
            id: Id::new("msg"),
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            role: role.into(),
            content,
            created_at: Utc::now(),
        };
        let content_json = serde_json::to_string(&message.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        // This compare-and-swap ends with an INSERT, so acquire the SQLite
        // write reservation before taking the source snapshot. A deferred
        // transaction can read successfully while another connection owns
        // the write reservation and then fail immediately when it upgrades
        // for the INSERT (SQLITE_BUSY), which made Undo intermittently return
        // an internal error while first-turn title persistence was running.
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=?")
            .bind(&session_id.0)
            .fetch_one(&mut *transaction)
            .await?;
        let latest: Option<String> = sqlx::query_scalar(
            "SELECT id FROM messages WHERE session_id=? ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(&session_id.0)
        .fetch_optional(&mut *transaction)
        .await?;
        if u64::try_from(count).unwrap_or(u64::MAX) != expected_message_count
            || latest.as_deref() != expected_latest_message_id.map(|id| id.0.as_str())
        {
            return Err(StorageError::InvalidState(
                "conversation changed while compaction was being prepared".into(),
            ));
        }
        sqlx::query("INSERT INTO messages (id,session_id,turn_id,role,content_json,created_at) VALUES (?,?,?,?,?,?)")
            .bind(&message.id.0)
            .bind(&message.session_id.0)
            .bind(&message.turn_id.0)
            .bind(&message.role)
            .bind(content_json)
            .bind(message.created_at)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(message.created_at)
            .bind(&session_id.0)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(message)
    }

    pub async fn append_reasoning_summary_delta(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        item_id: &Id,
        delta: &str,
    ) -> Result<Option<Message>, StorageError> {
        const MAX_DELTA_BYTES: usize = 8 * 1024;
        const MAX_SUMMARY_BYTES: usize = 64 * 1024;
        if delta.is_empty()
            || delta.len() > MAX_DELTA_BYTES
            || delta
                .chars()
                .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
        {
            return Err(StorageError::InvalidData(
                "reasoning summary delta must be non-empty, bounded text".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let existing = sqlx::query("SELECT * FROM messages WHERE id=?")
            .bind(&item_id.0)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = existing else {
            return self
                .append_turn_message_with_id(
                    scope,
                    session_id,
                    turn_id,
                    item_id.clone(),
                    "reasoning_summary_streaming",
                    serde_json::Value::String(delta.into()),
                )
                .await
                .map(Some);
        };
        let mut message = row_to_message(&row, scope, &self.sensitive)?;
        if message.session_id != *session_id
            || message.turn_id != *turn_id
            || message.role != "reasoning_summary_streaming"
        {
            return Err(StorageError::InvalidData(
                "reasoning summary identity collides with another message".into(),
            ));
        }
        let text = message.content.as_str().ok_or_else(|| {
            StorageError::InvalidData("reasoning summary content is not text".into())
        })?;
        if text.len().saturating_add(delta.len()) > MAX_SUMMARY_BYTES {
            return Ok(None);
        }
        message.content = serde_json::Value::String(format!("{text}{delta}"));
        let content_json = serde_json::to_string(&message.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        let changed = sqlx::query(
            "UPDATE messages SET content_json=? \
             WHERE id=? AND session_id=? AND turn_id=? AND role='reasoning_summary_streaming'",
        )
        .bind(content_json)
        .bind(&item_id.0)
        .bind(&session_id.0)
        .bind(&turn_id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidData(
                "reasoning summary changed concurrently".into(),
            ));
        }
        Ok(Some(message))
    }

    pub async fn complete_reasoning_summary(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_id: &Id,
        item_id: &Id,
    ) -> Result<bool, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let changed = sqlx::query(
            "UPDATE messages SET role='reasoning_summary' \
             WHERE id=? AND session_id=? AND turn_id=? AND role='reasoning_summary_streaming'",
        )
        .bind(&item_id.0)
        .bind(&session_id.0)
        .bind(&turn_id.0)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() == 1)
    }

    pub async fn list_messages(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<Message>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT * FROM messages WHERE session_id = ? ORDER BY created_at ASC,id ASC",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row_to_message(row, scope, &self.sensitive))
            .collect()
    }

    pub async fn list_transcript_item_references(
        &self,
        scope: &Scope,
        session_id: &Id,
        before: Option<(&DateTime<Utc>, &Id)>,
        limit: usize,
    ) -> Result<TranscriptItemReferencePage, StorageError> {
        if !(1..=1_000).contains(&limit) {
            return Err(StorageError::InvalidData(
                "transcript page limit must be between 1 and 1000".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let total_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM transcript_item_index WHERE session_id=?",
        )
        .bind(&session_id.0)
        .fetch_one(&self.pool)
        .await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT item_id,session_id,turn_id,source_kind,source_id,created_at \
             FROM transcript_item_index WHERE session_id=",
        );
        query.push_bind(&session_id.0);
        if let Some((created_at, item_id)) = before {
            query
                .push(" AND (created_at<")
                .push_bind(created_at)
                .push(" OR (created_at=")
                .push_bind(created_at)
                .push(" AND item_id<")
                .push_bind(&item_id.0)
                .push("))");
        }
        query
            .push(" ORDER BY created_at DESC,item_id DESC LIMIT ")
            .push_bind(i64::try_from(limit.saturating_add(1)).map_err(|_| {
                StorageError::InvalidData("transcript page limit exceeds SQLite range".into())
            })?);
        let rows = query.build().fetch_all(&self.pool).await?;
        let has_older = rows.len() > limit;
        let mut references = rows
            .iter()
            .take(limit)
            .map(row_to_transcript_item_reference)
            .collect::<Result<Vec<_>, _>>()?;
        references.reverse();
        Ok(TranscriptItemReferencePage {
            references,
            total_count: u64::try_from(total_count)
                .map_err(|_| StorageError::InvalidData("negative transcript item count".into()))?,
            has_older,
        })
    }

    pub async fn load_transcript_page_sources(
        &self,
        scope: &Scope,
        session_id: &Id,
        references: &[TranscriptItemReference],
    ) -> Result<TranscriptPageSources, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if references
            .iter()
            .any(|reference| reference.session_id != *session_id)
        {
            return Err(StorageError::InvalidData(
                "transcript page contains another session".into(),
            ));
        }

        let source_ids = |kind: TranscriptItemSourceKind| {
            references
                .iter()
                .filter(|reference| reference.source_kind == kind)
                .map(|reference| reference.source_id.0.clone())
                .collect::<Vec<_>>()
        };
        let message_ids = source_ids(TranscriptItemSourceKind::Message);
        let direct_tool_ids = source_ids(TranscriptItemSourceKind::ToolCall);
        let approval_ids = source_ids(TranscriptItemSourceKind::Approval);
        let question_ids = source_ids(TranscriptItemSourceKind::Question);
        let artifact_ids = source_ids(TranscriptItemSourceKind::Artifact);
        let plan_ids = source_ids(TranscriptItemSourceKind::Plan);
        let mut event_ids = source_ids(TranscriptItemSourceKind::ContextCompaction);
        event_ids.extend(source_ids(TranscriptItemSourceKind::ModelReroute));
        event_ids.extend(source_ids(TranscriptItemSourceKind::Usage));
        let agent_status_ids = source_ids(TranscriptItemSourceKind::AgentStatus);
        let hook_ids = source_ids(TranscriptItemSourceKind::Hook);

        let message_rows =
            select_session_rows_by_json_ids(&self.pool, "messages", session_id, &message_ids)
                .await?;
        let messages = message_rows
            .iter()
            .map(|row| row_to_message(row, scope, &self.sensitive))
            .collect::<Result<Vec<_>, _>>()?;

        let approval_rows =
            select_session_approval_rows_by_json_ids(&self.pool, session_id, &approval_ids).await?;
        let approvals = approval_rows
            .iter()
            .map(|row| {
                let approval = row_to_approval(row)?;
                ensure_actor_scope(&approval.scope, scope)?;
                Ok(approval)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let mut tool_ids = direct_tool_ids.clone();
        tool_ids.extend(
            approvals
                .iter()
                .map(|approval| approval.tool_call_id.0.clone()),
        );
        tool_ids.sort();
        tool_ids.dedup();
        let tool_rows =
            select_session_rows_by_json_ids(&self.pool, "tool_calls", session_id, &tool_ids)
                .await?;
        let tool_calls = tool_rows
            .iter()
            .map(|row| {
                let call = row_to_tool_call(row, &self.sensitive)?;
                ensure_actor_scope(&call.request.scope, scope)?;
                Ok(call)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        let question_rows = select_session_rows_by_json_ids(
            &self.pool,
            "question_requests",
            session_id,
            &question_ids,
        )
        .await?;
        let questions = question_rows
            .iter()
            .map(|row| {
                ensure_actor_scope(&row_scope(row)?, scope)?;
                row_to_question_request(row, &self.sensitive)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        let artifact_rows =
            select_session_rows_by_json_ids(&self.pool, "artifacts", session_id, &artifact_ids)
                .await?;
        let artifacts = artifact_rows
            .iter()
            .map(|row| {
                ensure_actor_scope(&row_scope(row)?, scope)?;
                row_to_artifact(row, &self.sensitive)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        let mut events =
            select_session_event_rows_by_json_ids(&self.pool, scope, session_id, &event_ids)
                .await?
                .iter()
                .map(|row| row_to_event(row, &self.sensitive))
                .collect::<Result<Vec<_>, _>>()?;
        let plan_rows =
            select_latest_plan_event_rows(&self.pool, scope, session_id, &plan_ids).await?;
        let plan_created_at = references
            .iter()
            .filter(|reference| reference.source_kind == TranscriptItemSourceKind::Plan)
            .map(|reference| (reference.source_id.0.as_str(), reference.created_at))
            .collect::<std::collections::HashMap<_, _>>();
        for row in &plan_rows {
            let mut event = row_to_event(row, &self.sensitive)?;
            if let Some(item_id) = event
                .payload
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                && let Some(created_at) = plan_created_at.get(item_id)
            {
                event.timestamp = *created_at;
            }
            events.push(event);
        }
        let agent_rows =
            select_latest_agent_status_event_rows(&self.pool, scope, session_id, &agent_status_ids)
                .await?;
        let agent_created_at = references
            .iter()
            .filter(|reference| reference.source_kind == TranscriptItemSourceKind::AgentStatus)
            .map(|reference| (reference.source_id.0.as_str(), reference.created_at))
            .collect::<std::collections::HashMap<_, _>>();
        for row in &agent_rows {
            let mut event = row_to_event(row, &self.sensitive)?;
            if let Some(item_id) = event
                .payload
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                && let Some(created_at) = agent_created_at.get(item_id)
            {
                event.timestamp = *created_at;
            }
            events.push(event);
        }
        let hook_rows =
            select_latest_hook_event_rows(&self.pool, scope, session_id, &hook_ids).await?;
        let hook_created_at = references
            .iter()
            .filter(|reference| reference.source_kind == TranscriptItemSourceKind::Hook)
            .map(|reference| (reference.source_id.0.as_str(), reference.created_at))
            .collect::<std::collections::HashMap<_, _>>();
        for row in &hook_rows {
            let mut event = row_to_event(row, &self.sensitive)?;
            if let Some(item_id) = event
                .payload
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                && let Some(created_at) = hook_created_at.get(item_id)
            {
                event.timestamp = *created_at;
            }
            events.push(event);
        }
        let progress_rows =
            select_latest_mcp_progress_event_rows(&self.pool, scope, session_id, &direct_tool_ids)
                .await?;
        events.extend(
            progress_rows
                .iter()
                .map(|row| row_to_event(row, &self.sensitive))
                .collect::<Result<Vec<_>, _>>()?,
        );

        Ok(TranscriptPageSources {
            messages,
            tool_calls,
            approvals,
            questions,
            artifacts,
            events,
        })
    }

    pub async fn create_attachment(
        &self,
        session_id: &Id,
        input: CreateAttachment,
    ) -> Result<Attachment, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        validate_attachment_name(&input.file_name)?;
        validate_attachment_media_type(&input.media_type)?;
        let bytes = STANDARD
            .decode(input.content_base64.as_bytes())
            .map_err(|_| StorageError::InvalidData("attachment is not valid base64".into()))?;
        if bytes.is_empty() {
            return Err(StorageError::InvalidData("attachment is empty".into()));
        }
        if bytes.len() > 5 * 1024 * 1024 {
            return Err(StorageError::InvalidData(
                "attachment exceeds the 5 MiB file limit".into(),
            ));
        }
        let draft_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM attachments
             WHERE session_id=? AND actor_id=? AND turn_id IS NULL",
        )
        .bind(&session_id.0)
        .bind(&input.scope.actor_id.0)
        .fetch_one(&self.pool)
        .await?;
        if draft_count >= 8 {
            return Err(StorageError::InvalidState(
                "at most eight draft attachments are allowed".into(),
            ));
        }
        let id = Id::new("attachment");
        let created_at = Utc::now();
        let canonical = STANDARD.encode(&bytes);
        let metadata = AttachmentMetadata {
            id: id.clone(),
            session_id: session_id.clone(),
            turn_id: None,
            file_name: input.file_name,
            media_type: input.media_type.to_ascii_lowercase(),
            byte_length: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            created_at,
        };
        let file_name = self.sensitive.seal_text(
            &input.scope,
            "attachments",
            &id,
            "file_name",
            &metadata.file_name,
        )?;
        let media_type = self.sensitive.seal_text(
            &input.scope,
            "attachments",
            &id,
            "media_type",
            &metadata.media_type,
        )?;
        let content_base64 = self.sensitive.seal_text(
            &input.scope,
            "attachments",
            &id,
            "content_base64",
            &canonical,
        )?;
        let sha256 = self.sensitive.seal_text(
            &input.scope,
            "attachments",
            &id,
            "sha256",
            &metadata.sha256,
        )?;
        sqlx::query(
            "INSERT INTO attachments
             (id,organization_id,team_id,actor_id,session_id,turn_id,file_name,media_type,content_base64,byte_length,sha256,created_at)
             VALUES (?,?,?,?,?,NULL,?,?,?,?,?,?)",
        )
        .bind(&id.0)
        .bind(&input.scope.organization_id.0)
        .bind(&input.scope.team_id.0)
        .bind(&input.scope.actor_id.0)
        .bind(&session_id.0)
        .bind(file_name)
        .bind(media_type)
        .bind(content_base64)
        .bind(metadata.byte_length as i64)
        .bind(sha256)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(Attachment {
            metadata,
            content_base64: canonical,
        })
    }

    pub async fn get_attachment(
        &self,
        scope: &Scope,
        attachment_id: &Id,
    ) -> Result<Attachment, StorageError> {
        let row = sqlx::query("SELECT * FROM attachments WHERE id=?")
            .bind(&attachment_id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        row_to_attachment(&row, scope, &self.sensitive)
    }

    pub async fn list_session_attachments(
        &self,
        scope: &Scope,
        session_id: &Id,
        include_drafts: bool,
    ) -> Result<Vec<AttachmentMetadata>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = if include_drafts {
            sqlx::query(
                "SELECT * FROM attachments
                 WHERE session_id=? AND (turn_id IS NOT NULL OR actor_id=?)
                 ORDER BY created_at,id",
            )
            .bind(&session_id.0)
            .bind(&scope.actor_id.0)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM attachments
                 WHERE session_id=? AND turn_id IS NOT NULL
                 ORDER BY created_at,id",
            )
            .bind(&session_id.0)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter()
            .map(|row| {
                row_to_attachment(row, scope, &self.sensitive).map(|attachment| attachment.metadata)
            })
            .collect()
    }

    pub async fn list_attachment_metadata_for_turns(
        &self,
        scope: &Scope,
        session_id: &Id,
        turn_ids: &[Id],
    ) -> Result<Vec<AttachmentMetadata>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if turn_ids.len() > 1_000 {
            return Err(StorageError::InvalidData(
                "attachment Turn page exceeds 1000 entries".into(),
            ));
        }
        if turn_ids.is_empty() {
            return Ok(Vec::new());
        }
        let turn_ids = serde_json::to_string(
            &turn_ids
                .iter()
                .map(|turn_id| turn_id.0.as_str())
                .collect::<Vec<_>>(),
        )
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let rows = sqlx::query(
            "SELECT * FROM attachments \
             WHERE session_id=? AND turn_id IN (SELECT value FROM json_each(?)) \
             ORDER BY created_at,id",
        )
        .bind(&session_id.0)
        .bind(turn_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                row_to_attachment(row, scope, &self.sensitive).map(|attachment| attachment.metadata)
            })
            .collect()
    }

    pub async fn list_turn_attachments(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Vec<Attachment>, StorageError> {
        let turn = self.get_turn(scope, turn_id).await?;
        let rows = sqlx::query("SELECT * FROM attachments WHERE turn_id=? ORDER BY created_at,id")
            .bind(&turn.id.0)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| row_to_attachment(row, scope, &self.sensitive))
            .collect()
    }

    pub async fn delete_draft_attachment(
        &self,
        scope: &Scope,
        attachment_id: &Id,
    ) -> Result<(), StorageError> {
        let deleted = sqlx::query(
            "DELETE FROM attachments
             WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND turn_id IS NULL",
        )
        .bind(&attachment_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .execute(&self.pool)
        .await?;
        if deleted.rows_affected() == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    pub async fn list_turns(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<Turn>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query("SELECT * FROM turns WHERE session_id = ? ORDER BY started_at ASC")
            .bind(&session_id.0)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                let turn = row_to_turn(row, &self.sensitive)?;
                ensure_actor_scope(&turn.scope, scope)?;
                Ok(turn)
            })
            .collect()
    }

    pub async fn create_turn(&self, scope: &Scope, session_id: &Id) -> Result<Turn, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let now = Utc::now();
        let turn = Turn {
            id: Id::new("turn"),
            session_id: session_id.clone(),
            scope: scope.clone(),
            status: TurnStatus::Idle,
            checkpoint: None,
            error_code: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };
        let inserted = sqlx::query(
            "INSERT INTO turns (id,session_id,organization_id,team_id,actor_id,goal_id,task_id,status,started_at,updated_at)
             SELECT ?,?,?,?,?,?,?,'idle',?,?
             WHERE EXISTS (
                 SELECT 1 FROM sessions
                 WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status='active'
             )",
        )
            .bind(&turn.id.0).bind(&turn.session_id.0).bind(&turn.scope.organization_id.0).bind(&turn.scope.team_id.0).bind(&turn.scope.actor_id.0)
            .bind(turn.scope.goal_id.as_ref().map(|id| &id.0)).bind(turn.scope.task_id.as_ref().map(|id| &id.0)).bind(now).bind(now)
            .bind(&turn.session_id.0).bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&scope.actor_id.0)
            .execute(&self.pool).await?;
        if inserted.rows_affected() != 1 {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        Ok(turn)
    }

    pub async fn create_turn_with_user_message_if_idle(
        &self,
        scope: &Scope,
        session_id: &Id,
        content: serde_json::Value,
    ) -> Result<(Turn, Message), StorageError> {
        self.create_turn_with_user_message_and_attachments_if_idle(scope, session_id, content, &[])
            .await
    }

    pub async fn create_turn_with_user_message_and_attachments_if_idle(
        &self,
        scope: &Scope,
        session_id: &Id,
        content: serde_json::Value,
        attachment_ids: &[Id],
    ) -> Result<(Turn, Message), StorageError> {
        self.create_turn_with_message_and_attachments_if_idle(
            scope,
            session_id,
            "user",
            content,
            attachment_ids,
        )
        .await
    }

    pub async fn create_turn_with_system_message_if_idle(
        &self,
        scope: &Scope,
        session_id: &Id,
        content: serde_json::Value,
    ) -> Result<(Turn, Message), StorageError> {
        self.create_turn_with_message_and_attachments_if_idle(
            scope,
            session_id,
            "system",
            content,
            &[],
        )
        .await
    }

    async fn create_turn_with_message_and_attachments_if_idle(
        &self,
        scope: &Scope,
        session_id: &Id,
        role: &str,
        content: serde_json::Value,
        attachment_ids: &[Id],
    ) -> Result<(Turn, Message), StorageError> {
        if !matches!(role, "user" | "system") {
            return Err(StorageError::InvalidData(
                "turn message role must be user or system".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        if attachment_ids.len() > 8 {
            return Err(StorageError::InvalidData(
                "a turn can contain at most eight attachments".into(),
            ));
        }
        let unique = attachment_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<HashSet<_>>();
        if unique.len() != attachment_ids.len() {
            return Err(StorageError::InvalidData(
                "attachment IDs must be unique".into(),
            ));
        }
        let now = Utc::now();
        let turn = Turn {
            id: Id::new("turn"),
            session_id: session_id.clone(),
            scope: scope.clone(),
            status: TurnStatus::Idle,
            checkpoint: None,
            error_code: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };
        let message = Message {
            id: Id::new("msg"),
            session_id: session_id.clone(),
            turn_id: turn.id.clone(),
            role: role.into(),
            content,
            created_at: now,
        };
        let content_json = serde_json::to_string(&message.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO turns (id,session_id,organization_id,team_id,actor_id,goal_id,task_id,status,started_at,updated_at)
             SELECT ?,?,?,?,?,?,?,'idle',?,?
             WHERE EXISTS (
                 SELECT 1 FROM sessions
                 WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status='active'
             )
             AND NOT EXISTS (
                 SELECT 1 FROM turns
                 WHERE session_id=? AND status NOT IN ('completed','failed','cancelled')
             )
             AND NOT EXISTS (
                 SELECT 1 FROM turn_inputs
                 WHERE session_id=? AND status IN ('pending','processing')
             )",
        )
        .bind(&turn.id.0)
        .bind(&turn.session_id.0)
        .bind(&turn.scope.organization_id.0)
        .bind(&turn.scope.team_id.0)
        .bind(&turn.scope.actor_id.0)
        .bind(turn.scope.goal_id.as_ref().map(|id| &id.0))
        .bind(turn.scope.task_id.as_ref().map(|id| &id.0))
        .bind(now)
        .bind(now)
        .bind(&turn.session_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(&turn.session_id.0)
        .bind(&turn.session_id.0)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "session is not active or already has active or queued input".into(),
            ));
        }
        sqlx::query("INSERT INTO messages (id,session_id,turn_id,role,content_json,created_at) VALUES (?,?,?,?,?,?)")
            .bind(&message.id.0)
            .bind(&message.session_id.0)
            .bind(&message.turn_id.0)
            .bind(&message.role)
            .bind(content_json)
            .bind(message.created_at)
            .execute(&mut *transaction)
            .await?;
        for attachment_id in attachment_ids {
            let attached = sqlx::query(
                "UPDATE attachments SET turn_id=?
                 WHERE id=? AND organization_id=? AND team_id=? AND actor_id=?
                   AND session_id=? AND turn_id IS NULL",
            )
            .bind(&turn.id.0)
            .bind(&attachment_id.0)
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&scope.actor_id.0)
            .bind(&session_id.0)
            .execute(&mut *transaction)
            .await?;
            if attached.rows_affected() != 1 {
                return Err(StorageError::InvalidState(format!(
                    "attachment {} is unavailable",
                    attachment_id.0
                )));
            }
        }
        let updated = sqlx::query(
            "UPDATE sessions SET updated_at=?
             WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND status='active'",
        )
        .bind(now)
        .bind(&session_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        transaction.commit().await?;
        Ok((turn, message))
    }

    pub async fn create_turn_input(
        &self,
        session_id: &Id,
        input: CreateTurnInput,
    ) -> Result<TurnInput, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let target = self.get_turn(&input.scope, &input.target_turn_id).await?;
        if target.session_id != *session_id {
            return Err(StorageError::InvalidState(
                "target turn belongs to a different session".into(),
            ));
        }
        if is_terminal(&target.status) {
            return Err(StorageError::InvalidState(
                "target turn is already terminal".into(),
            ));
        }
        let content = input
            .content
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StorageError::InvalidData("turn input must be text".into()))?;
        if content.chars().count() > 100_000 {
            return Err(StorageError::InvalidData("turn input is too large".into()));
        }
        if input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 128
            || !input
                .idempotency_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(StorageError::InvalidData(
                "invalid turn input idempotency key".into(),
            ));
        }
        let now = Utc::now();
        let turn_input = TurnInput {
            id: Id::new("input"),
            session_id: session_id.clone(),
            target_turn_id: input.target_turn_id,
            resulting_turn_id: None,
            scope: input.scope,
            mode: input.mode,
            content: input.content,
            status: TurnInputStatus::Pending,
            idempotency_key: input.idempotency_key,
            created_at: now,
            updated_at: now,
            consumed_at: None,
            cancelled_at: None,
            revision: 1,
        };
        let content_json = serde_json::to_string(&turn_input.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            &turn_input.scope,
            "turn_inputs",
            &turn_input.id,
            "content_json",
            &content_json,
        )?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO turn_inputs
             (id,organization_id,team_id,actor_id,goal_id,task_id,session_id,target_turn_id,resulting_turn_id,mode,content_json,status,idempotency_key,created_at,updated_at,revision)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,'pending',?,?,?,1)",
        )
        .bind(&turn_input.id.0)
        .bind(&turn_input.scope.organization_id.0)
        .bind(&turn_input.scope.team_id.0)
        .bind(&turn_input.scope.actor_id.0)
        .bind(turn_input.scope.goal_id.as_ref().map(|id| &id.0))
        .bind(turn_input.scope.task_id.as_ref().map(|id| &id.0))
        .bind(&turn_input.session_id.0)
        .bind(&turn_input.target_turn_id.0)
        .bind(turn_input.resulting_turn_id.as_ref().map(|id| &id.0))
        .bind(turn_input_mode_str(&turn_input.mode))
        .bind(content_json)
        .bind(&turn_input.idempotency_key)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok(turn_input);
        }
        let existing = sqlx::query(
            "SELECT * FROM turn_inputs
             WHERE organization_id=? AND team_id=? AND idempotency_key=?",
        )
        .bind(&turn_input.scope.organization_id.0)
        .bind(&turn_input.scope.team_id.0)
        .bind(&turn_input.idempotency_key)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let existing = row_to_turn_input(&row, &self.sensitive)?;
            if existing.scope.actor_id == turn_input.scope.actor_id
                && existing.session_id == turn_input.session_id
                && existing.target_turn_id == turn_input.target_turn_id
                && existing.mode == turn_input.mode
                && existing.content == turn_input.content
            {
                return Ok(existing);
            }
            return Err(StorageError::InvalidState(
                "turn input idempotency key was reused".into(),
            ));
        }
        Err(StorageError::InvalidState(
            "a steering input is already pending for this turn".into(),
        ))
    }

    pub async fn list_pending_turn_inputs(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<TurnInput>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT * FROM turn_inputs
             WHERE session_id=? AND status IN ('pending','processing')
             ORDER BY created_at,id LIMIT 1001",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > 1_000 {
            return Err(StorageError::InvalidData(
                "session has more than 1000 pending Turn inputs".into(),
            ));
        }
        rows.iter()
            .map(|row| {
                let input = row_to_turn_input(row, &self.sensitive)?;
                ensure_actor_scope(&input.scope, scope)?;
                Ok(input)
            })
            .collect()
    }

    pub async fn get_turn_input(&self, scope: &Scope, id: &Id) -> Result<TurnInput, StorageError> {
        let row = sqlx::query("SELECT * FROM turn_inputs WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let input = row_to_turn_input(&row, &self.sensitive)?;
        ensure_actor_scope(&input.scope, scope)?;
        Ok(input)
    }

    pub async fn cancel_turn_input(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<TurnInput, StorageError> {
        let current = self.get_turn_input(scope, id).await?;
        if current.status == TurnInputStatus::Cancelled {
            return Ok(current);
        }
        if current.status != TurnInputStatus::Pending {
            return Err(StorageError::InvalidState(
                "turn input can no longer be cancelled".into(),
            ));
        }
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE turn_inputs
             SET status='cancelled',updated_at=?,cancelled_at=?,revision=revision+1
             WHERE id=? AND status='pending'",
        )
        .bind(now)
        .bind(now)
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "turn input changed concurrently".into(),
            ));
        }
        self.get_turn_input(scope, id).await
    }

    /// Atomically removes every still-pending Prompt Queue entry anchored to
    /// a Turn from conversation time. A worker-owned entry fails closed so an
    /// Undo cannot race creation of its successor Turn.
    pub async fn cancel_turn_inputs_for_undo(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Vec<TurnInput>, StorageError> {
        let turn = self.get_turn(scope, turn_id).await?;
        let session = self.get_session(&turn.session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(
            "UPDATE turn_inputs
             SET status='cancelled',updated_at=?,cancelled_at=?,revision=revision+1
             WHERE target_turn_id=? AND status='pending'
             RETURNING *",
        )
        .bind(now)
        .bind(now)
        .bind(&turn_id.0)
        .fetch_all(&mut *transaction)
        .await?;
        let processing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM turn_inputs
             WHERE target_turn_id=? AND status='processing'",
        )
        .bind(&turn_id.0)
        .fetch_one(&mut *transaction)
        .await?;
        if processing != 0 {
            transaction.rollback().await?;
            return Err(StorageError::InvalidState(
                "a queued Turn input is already being consumed".into(),
            ));
        }
        let cancelled = rows
            .iter()
            .map(|row| row_to_turn_input(row, &self.sensitive))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.commit().await?;
        for input in &cancelled {
            ensure_actor_scope(&input.scope, scope)?;
        }
        Ok(cancelled)
    }

    pub async fn claim_next_ready_turn_input(&self) -> Result<Option<TurnInput>, StorageError> {
        let stale_before = Utc::now() - chrono::Duration::seconds(60);
        sqlx::query(
            "UPDATE turn_inputs
             SET status='pending',updated_at=?,revision=revision+1
             WHERE status='processing' AND resulting_turn_id IS NULL AND updated_at<?",
        )
        .bind(Utc::now())
        .bind(stale_before)
        .execute(&self.pool)
        .await?;
        let now = Utc::now();
        let row = sqlx::query(
            "UPDATE turn_inputs
             SET status='processing',updated_at=?,revision=revision+1
             WHERE id=(
                 SELECT input.id
                 FROM turn_inputs input
                 JOIN sessions session ON session.id=input.session_id
                 JOIN turns target ON target.id=input.target_turn_id
                 WHERE input.status='pending'
                   AND session.status='active'
                   AND target.status IN ('completed','failed','cancelled')
                   AND NOT EXISTS (
                       SELECT 1 FROM turns active
                       WHERE active.session_id=input.session_id
                         AND active.status NOT IN ('completed','failed','cancelled')
                   )
                 ORDER BY CASE input.mode WHEN 'steer' THEN 0 ELSE 1 END,
                          input.created_at,input.id
                 LIMIT 1
             )
             RETURNING *",
        )
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref()
            .map(|row| row_to_turn_input(row, &self.sensitive))
            .transpose()
    }

    pub async fn requeue_turn_input(&self, id: &Id) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE turn_inputs
             SET status='pending',updated_at=?,revision=revision+1
             WHERE id=? AND status='processing' AND resulting_turn_id IS NULL",
        )
        .bind(Utc::now())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Materializes a persistent StepRequest into its still-active Turn. The
    /// message and input transition commit together before model context sees
    /// the request.
    pub async fn consume_turn_input_into_active_turn(
        &self,
        id: &Id,
        turn_id: &Id,
    ) -> Result<(TurnInput, Message), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM turn_inputs WHERE id=? AND status='pending'")
            .bind(&id.0)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| StorageError::InvalidState("turn input is not pending".into()))?;
        let mut input = row_to_turn_input(&row, &self.sensitive)?;
        if input.target_turn_id != *turn_id {
            return Err(StorageError::InvalidState(
                "turn input targets a different active Turn".into(),
            ));
        }
        let turn_row = sqlx::query("SELECT * FROM turns WHERE id=?")
            .bind(&turn_id.0)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::NotFound)?;
        let turn = row_to_turn(&turn_row, &self.sensitive)?;
        ensure_actor_scope(&turn.scope, &input.scope)?;
        if turn.session_id != input.session_id || is_terminal(&turn.status) {
            return Err(StorageError::InvalidState(
                "target Turn is no longer active".into(),
            ));
        }
        let now = Utc::now();
        let message = Message {
            id: Id::new("msg"),
            session_id: input.session_id.clone(),
            turn_id: turn_id.clone(),
            role: "user".into(),
            content: input.content.clone(),
            created_at: now,
        };
        let content_json = serde_json::to_string(&message.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            &input.scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        sqlx::query(
            "INSERT INTO messages (id,session_id,turn_id,role,content_json,created_at)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(&message.id.0)
        .bind(&message.session_id.0)
        .bind(&message.turn_id.0)
        .bind(&message.role)
        .bind(content_json)
        .bind(message.created_at)
        .execute(&mut *transaction)
        .await?;
        let changed = sqlx::query(
            "UPDATE turn_inputs
             SET status='consumed',resulting_turn_id=?,updated_at=?,consumed_at=?,revision=revision+1
             WHERE id=? AND status='pending'
               AND EXISTS (
                   SELECT 1 FROM turns
                   WHERE id=? AND status NOT IN ('completed','failed','cancelled')
               )",
        )
        .bind(&turn_id.0)
        .bind(now)
        .bind(now)
        .bind(&input.id.0)
        .bind(&turn_id.0)
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "turn input or active Turn changed concurrently".into(),
            ));
        }
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(now)
            .bind(&input.session_id.0)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        input.status = TurnInputStatus::Consumed;
        input.resulting_turn_id = Some(turn_id.clone());
        input.updated_at = now;
        input.consumed_at = Some(now);
        input.revision = input.revision.saturating_add(1);
        Ok((input, message))
    }

    pub async fn consume_turn_input_into_turn(
        &self,
        id: &Id,
    ) -> Result<(TurnInput, Turn, Message), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM turn_inputs WHERE id=? AND status='processing'")
            .bind(&id.0)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| StorageError::InvalidState("turn input is not processing".into()))?;
        let mut input = row_to_turn_input(&row, &self.sensitive)?;
        let session_row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(&input.session_id.0)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::NotFound)?;
        let session = row_to_session(&session_row, &self.sensitive)?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if session.status != SessionStatus::Active {
            return Err(StorageError::InvalidState("session is not active".into()));
        }
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM turns
             WHERE session_id=? AND status NOT IN ('completed','failed','cancelled')",
        )
        .bind(&input.session_id.0)
        .fetch_one(&mut *transaction)
        .await?;
        if active != 0 {
            return Err(StorageError::InvalidState(
                "session has another active turn".into(),
            ));
        }
        let now = Utc::now();
        let turn = Turn {
            id: Id::new("turn"),
            session_id: input.session_id.clone(),
            scope: input.scope.clone(),
            status: TurnStatus::Idle,
            checkpoint: None,
            error_code: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };
        let message = Message {
            id: Id::new("msg"),
            session_id: input.session_id.clone(),
            turn_id: turn.id.clone(),
            role: "user".into(),
            content: input.content.clone(),
            created_at: now,
        };
        let content_json = serde_json::to_string(&message.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content_json = self.sensitive.seal_text(
            &input.scope,
            "messages",
            &message.id,
            "content_json",
            &content_json,
        )?;
        sqlx::query("INSERT INTO turns (id,session_id,organization_id,team_id,actor_id,goal_id,task_id,status,started_at,updated_at) VALUES (?,?,?,?,?,?,?,'idle',?,?)")
            .bind(&turn.id.0)
            .bind(&turn.session_id.0)
            .bind(&turn.scope.organization_id.0)
            .bind(&turn.scope.team_id.0)
            .bind(&turn.scope.actor_id.0)
            .bind(turn.scope.goal_id.as_ref().map(|id| &id.0))
            .bind(turn.scope.task_id.as_ref().map(|id| &id.0))
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        sqlx::query("INSERT INTO messages (id,session_id,turn_id,role,content_json,created_at) VALUES (?,?,?,?,?,?)")
            .bind(&message.id.0)
            .bind(&message.session_id.0)
            .bind(&message.turn_id.0)
            .bind(&message.role)
            .bind(content_json)
            .bind(message.created_at)
            .execute(&mut *transaction)
            .await?;
        let changed = sqlx::query(
            "UPDATE turn_inputs
             SET status='consumed',resulting_turn_id=?,updated_at=?,consumed_at=?,revision=revision+1
             WHERE id=? AND status='processing'",
        )
        .bind(&turn.id.0)
        .bind(now)
        .bind(now)
        .bind(&input.id.0)
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "turn input changed concurrently".into(),
            ));
        }
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(now)
            .bind(&input.session_id.0)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        input.status = TurnInputStatus::Consumed;
        input.resulting_turn_id = Some(turn.id.clone());
        input.updated_at = now;
        input.consumed_at = Some(now);
        input.revision = input.revision.saturating_add(1);
        Ok((input, turn, message))
    }

    pub async fn get_turn(&self, scope: &Scope, id: &Id) -> Result<Turn, StorageError> {
        let row = sqlx::query("SELECT * FROM turns WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let turn = row_to_turn(&row, &self.sensitive)?;
        ensure_actor_scope(&turn.scope, scope)?;
        Ok(turn)
    }

    pub async fn update_turn(
        &self,
        scope: &Scope,
        id: &Id,
        status: TurnStatus,
        checkpoint: Option<&serde_json::Value>,
        error_code: Option<&str>,
    ) -> Result<Turn, StorageError> {
        let current = self.get_turn(scope, id).await?;
        if is_terminal(&current.status) {
            return Err(StorageError::InvalidState(
                "turn is already terminal".into(),
            ));
        }
        let now = Utc::now();
        let checkpoint_json = checkpoint
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let checkpoint_json = checkpoint_json
            .map(|value| {
                self.sensitive
                    .seal_text(scope, "turns", id, "checkpoint_json", &value)
            })
            .transpose()?;
        let completed_at = is_terminal(&status).then_some(now);
        sqlx::query("UPDATE turns SET status = ?, checkpoint_json = ?, error_code = ?, updated_at = ?, completed_at = ? WHERE id = ?")
            .bind(turn_status_str(&status)).bind(checkpoint_json).bind(error_code).bind(now).bind(completed_at).bind(&id.0).execute(&self.pool).await?;
        self.get_turn(scope, id).await
    }

    pub async fn plan_turn_file_change(
        &self,
        scope: &Scope,
        turn_id: &Id,
        path: &str,
        before_content: Option<&[u8]>,
        before_sha256: Option<&str>,
        after_sha256: &str,
    ) -> Result<TurnFileChangePlan, StorageError> {
        let turn = self.get_turn(scope, turn_id).await?;
        if path.is_empty()
            || after_sha256.len() != 64
            || before_content.is_some() != before_sha256.is_some()
        {
            return Err(StorageError::InvalidData(
                "file change requires a path, hashes and a consistent before-image".into(),
            ));
        }
        let operation_id = Id::new("fileop");
        let now = Utc::now();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query(
            "SELECT after_sha256,state FROM turn_file_changes WHERE turn_id=? AND path=?",
        )
        .bind(&turn.id.0)
        .bind(path)
        .fetch_optional(&mut *tx)
        .await?;
        let (previous_after_sha256, was_new) = if let Some(row) = existing {
            let state: String = row.try_get("state")?;
            if state == "undone" {
                return Err(StorageError::InvalidState(
                    "cannot append to an already undone Turn file change".into(),
                ));
            }
            if state == "planned" {
                return Err(StorageError::InvalidState(
                    "another file mutation is still planned".into(),
                ));
            }
            let previous: String = row.try_get("after_sha256")?;
            sqlx::query("UPDATE turn_file_changes SET after_sha256=?,previous_after_sha256=?,state='planned',operation_id=?,updated_at=? WHERE turn_id=? AND path=?")
                .bind(after_sha256).bind(&previous).bind(&operation_id.0).bind(now).bind(&turn.id.0).bind(path).execute(&mut *tx).await?;
            (Some(previous), false)
        } else {
            let sealed_before = before_content
                .map(|value| {
                    self.sensitive
                        .seal(scope, "turn_file_changes", &turn.id, path, value)
                })
                .transpose()?;
            sqlx::query("INSERT INTO turn_file_changes(turn_id,session_id,path,before_exists,before_content,before_sha256,after_sha256,state,operation_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'planned',?,?,?)")
                .bind(&turn.id.0).bind(&turn.session_id.0).bind(path).bind(before_content.is_some()).bind(sealed_before)
                .bind(before_sha256).bind(after_sha256).bind(&operation_id.0).bind(now).bind(now).execute(&mut *tx).await?;
            (None, true)
        };
        tx.commit().await?;
        Ok(TurnFileChangePlan {
            operation_id,
            turn_id: turn.id,
            path: path.into(),
            previous_after_sha256,
            was_new,
        })
    }

    pub async fn complete_turn_file_change(
        &self,
        plan: &TurnFileChangePlan,
    ) -> Result<(), StorageError> {
        let updated = sqlx::query("UPDATE turn_file_changes SET state='applied',operation_id=NULL,previous_after_sha256=NULL,updated_at=? WHERE turn_id=? AND path=? AND state='planned' AND operation_id=?")
            .bind(Utc::now()).bind(&plan.turn_id.0).bind(&plan.path).bind(&plan.operation_id.0).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "file mutation plan is no longer current".into(),
            ));
        }
        Ok(())
    }

    pub async fn abort_turn_file_change(
        &self,
        plan: &TurnFileChangePlan,
    ) -> Result<(), StorageError> {
        let result = if plan.was_new {
            sqlx::query("DELETE FROM turn_file_changes WHERE turn_id=? AND path=? AND state='planned' AND operation_id=?")
                .bind(&plan.turn_id.0).bind(&plan.path).bind(&plan.operation_id.0).execute(&self.pool).await?
        } else {
            sqlx::query("UPDATE turn_file_changes SET after_sha256=?,previous_after_sha256=NULL,state='applied',operation_id=NULL,updated_at=? WHERE turn_id=? AND path=? AND state='planned' AND operation_id=?")
                .bind(plan.previous_after_sha256.as_deref()).bind(Utc::now()).bind(&plan.turn_id.0).bind(&plan.path).bind(&plan.operation_id.0).execute(&self.pool).await?
        };
        if result.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "file mutation plan is no longer current".into(),
            ));
        }
        Ok(())
    }

    pub async fn list_turn_file_changes(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Vec<TurnFileChange>, StorageError> {
        self.get_turn(scope, turn_id).await?;
        let rows = sqlx::query("SELECT * FROM turn_file_changes WHERE turn_id=? AND state IN ('planned','applied') ORDER BY updated_at DESC,path DESC")
            .bind(&turn_id.0).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok(TurnFileChange {
                    turn_id: Id(row.try_get("turn_id")?),
                    session_id: Id(row.try_get("session_id")?),
                    path: row.try_get("path")?,
                    before_content: row
                        .try_get::<Option<Vec<u8>>, _>("before_content")?
                        .map(|value| {
                            self.sensitive.open(
                                scope,
                                "turn_file_changes",
                                turn_id,
                                &row.try_get::<String, _>("path")?,
                                &value,
                            )
                        })
                        .transpose()?,
                    before_sha256: row.try_get("before_sha256")?,
                    after_sha256: row.try_get("after_sha256")?,
                    previous_after_sha256: row.try_get("previous_after_sha256")?,
                    state: row.try_get("state")?,
                })
            })
            .collect()
    }

    pub async fn mark_turn_file_change_undone(
        &self,
        scope: &Scope,
        turn_id: &Id,
        path: &str,
    ) -> Result<(), StorageError> {
        self.get_turn(scope, turn_id).await?;
        let now = Utc::now();
        let updated = sqlx::query("UPDATE turn_file_changes SET state='undone',operation_id=NULL,updated_at=?,undone_at=? WHERE turn_id=? AND path=? AND state IN ('planned','applied')")
            .bind(now).bind(now).bind(&turn_id.0).bind(path).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Turn file change was already undone or does not exist".into(),
            ));
        }
        Ok(())
    }

    pub async fn create_background_terminal(
        &self,
        scope: &Scope,
        terminal: &BackgroundTerminalSpec,
        permission_digest: &str,
        turn_id: &Id,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        let session = self.get_session(&terminal.session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let turn = self.get_turn(scope, turn_id).await?;
        if turn.session_id != terminal.session_id {
            return Err(StorageError::InvalidState(
                "background terminal Turn does not belong to its Session".into(),
            ));
        }
        if permission_digest.len() != 64
            || !permission_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(StorageError::InvalidData(
                "background terminal permission digest is invalid".into(),
            ));
        }
        let id = Id::new("term");
        let now = Utc::now();
        let spec_json = serde_json::to_string(terminal)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let spec_json = self.sensitive.seal_text(
            scope,
            "background_terminals",
            &id,
            "spec_json",
            &spec_json,
        )?;
        let program = self.sensitive.seal_text(
            scope,
            "background_terminals",
            &id,
            "program",
            &terminal.program,
        )?;
        let working_directory_uri = self.sensitive.seal_text(
            scope,
            "background_terminals",
            &id,
            "working_directory_uri",
            &terminal.working_directory_uri,
        )?;
        let output_base64 =
            self.sensitive
                .seal_text(scope, "background_terminals", &id, "output_base64", "")?;
        sqlx::query(
            "INSERT INTO background_terminals
             (id,organization_id,team_id,actor_id,goal_id,task_id,session_id,turn_id,
              spec_json,permission_digest,program,argument_count,working_directory_uri,status,
              rows,cols,max_runtime_seconds,output_base64,created_at,updated_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,'starting',?,?,?,?,?,?)",
        )
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(scope.goal_id.as_ref().map(|value| &value.0))
        .bind(scope.task_id.as_ref().map(|value| &value.0))
        .bind(&terminal.session_id.0)
        .bind(&turn_id.0)
        .bind(spec_json)
        .bind(permission_digest)
        .bind(program)
        .bind(i64::try_from(terminal.args.len()).map_err(|_| {
            StorageError::InvalidData("background terminal has too many arguments".into())
        })?)
        .bind(working_directory_uri)
        .bind(i64::from(terminal.rows))
        .bind(i64::from(terminal.cols))
        .bind(i64::try_from(terminal.max_runtime_seconds).map_err(|_| {
            StorageError::InvalidData("background terminal runtime is too large".into())
        })?)
        .bind(output_base64)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_background_terminal(scope, &id).await
    }

    pub async fn get_background_terminal(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        let row = sqlx::query("SELECT * FROM background_terminals WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        ensure_actor_scope(&row_scope(&row)?, scope)?;
        row_to_background_terminal(&row, &self.sensitive)
    }

    pub async fn get_background_terminal_spec(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<BackgroundTerminalSpec, StorageError> {
        let row = sqlx::query("SELECT * FROM background_terminals WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let stored_scope = row_scope(&row)?;
        ensure_actor_scope(&stored_scope, scope)?;
        let value = self.sensitive.open_text(
            &stored_scope,
            "background_terminals",
            id,
            "spec_json",
            &row.try_get::<String, _>("spec_json")?,
        )?;
        serde_json::from_str(&value).map_err(|error| StorageError::InvalidData(error.to_string()))
    }

    pub async fn list_background_terminals(
        &self,
        scope: &Scope,
    ) -> Result<Vec<BackgroundTerminalSummary>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM background_terminals
             WHERE organization_id=? AND team_id=? AND actor_id=?
             ORDER BY updated_at DESC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row_to_background_terminal(row, &self.sensitive))
            .collect()
    }

    pub async fn set_background_terminal_running(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        self.get_background_terminal(scope, id).await?;
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE background_terminals
             SET status='running',started_at=COALESCE(started_at,?),updated_at=?,revision=revision+1
             WHERE id=? AND status='starting'",
        )
        .bind(now)
        .bind(now)
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "background terminal is not starting".into(),
            ));
        }
        self.get_background_terminal(scope, id).await
    }

    pub async fn append_background_terminal_output(
        &self,
        scope: &Scope,
        id: &Id,
        chunk: &[u8],
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
        if chunk.is_empty() {
            return self.get_background_terminal(scope, id).await;
        }
        let row = sqlx::query("SELECT * FROM background_terminals WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let stored_scope = row_scope(&row)?;
        ensure_actor_scope(&stored_scope, scope)?;
        let current = self.sensitive.open_text(
            &stored_scope,
            "background_terminals",
            id,
            "output_base64",
            &row.try_get::<String, _>("output_base64")?,
        )?;
        let mut output = if current.is_empty() {
            Vec::new()
        } else {
            STANDARD
                .decode(current)
                .map_err(|_| StorageError::InvalidData("terminal output is malformed".into()))?
        };
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
        output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        let previous_total = u64::try_from(row.try_get::<i64, _>("output_byte_length")?)
            .map_err(|_| StorageError::InvalidData("negative terminal output length".into()))?;
        let total = previous_total.saturating_add(chunk.len() as u64);
        let truncated = row.try_get::<bool, _>("output_truncated")? || chunk.len() > remaining;
        let encoded = STANDARD.encode(output);
        let encoded = self.sensitive.seal_text(
            &stored_scope,
            "background_terminals",
            id,
            "output_base64",
            &encoded,
        )?;
        let now = Utc::now();
        sqlx::query(
            "UPDATE background_terminals
             SET output_base64=?,output_byte_length=?,output_truncated=?,updated_at=?,revision=revision+1
             WHERE id=? AND status IN ('starting','running')",
        )
        .bind(encoded)
        .bind(i64::try_from(total).unwrap_or(i64::MAX))
        .bind(truncated)
        .bind(now)
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        self.get_background_terminal(scope, id).await
    }

    pub async fn replace_background_terminal_output_snapshot(
        &self,
        scope: &Scope,
        id: &Id,
        snapshot: &[u8],
        observed_bytes: u64,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
        let current = self.get_background_terminal(scope, id).await?;
        let retained = &snapshot[..snapshot.len().min(MAX_OUTPUT_BYTES)];
        let encoded = self.sensitive.seal_text(
            scope,
            "background_terminals",
            id,
            "output_base64",
            &STANDARD.encode(retained),
        )?;
        let observed = observed_bytes.max(current.output_byte_length);
        sqlx::query(
            "UPDATE background_terminals \
             SET output_base64=?,output_byte_length=?,output_truncated=?,updated_at=?,revision=revision+1 \
             WHERE id=? AND status IN ('starting','running')",
        )
        .bind(encoded)
        .bind(i64::try_from(observed).unwrap_or(i64::MAX))
        .bind(current.output_truncated || observed > retained.len() as u64 || snapshot.len() > retained.len())
        .bind(Utc::now())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        self.get_background_terminal(scope, id).await
    }

    pub async fn finalize_background_terminal_output(
        &self,
        scope: &Scope,
        id: &Id,
        observed_bytes: u64,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        const MAX_OUTPUT_BYTES: u64 = 1024 * 1024;
        let current = self.get_background_terminal(scope, id).await?;
        let observed = observed_bytes.max(current.output_byte_length);
        sqlx::query(
            "UPDATE background_terminals \
             SET output_byte_length=?,output_truncated=?,updated_at=?,revision=revision+1 \
             WHERE id=? AND status IN ('starting','running')",
        )
        .bind(i64::try_from(observed).unwrap_or(i64::MAX))
        .bind(current.output_truncated || observed > MAX_OUTPUT_BYTES)
        .bind(Utc::now())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        self.get_background_terminal(scope, id).await
    }

    pub async fn get_background_terminal_output(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<Vec<u8>, StorageError> {
        let row = sqlx::query("SELECT * FROM background_terminals WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let stored_scope = row_scope(&row)?;
        ensure_actor_scope(&stored_scope, scope)?;
        let encoded = self.sensitive.open_text(
            &stored_scope,
            "background_terminals",
            id,
            "output_base64",
            &row.try_get::<String, _>("output_base64")?,
        )?;
        if encoded.is_empty() {
            Ok(Vec::new())
        } else {
            STANDARD
                .decode(encoded)
                .map_err(|_| StorageError::InvalidData("terminal output is malformed".into()))
        }
    }

    pub async fn resize_background_terminal(
        &self,
        scope: &Scope,
        id: &Id,
        rows: u16,
        cols: u16,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        self.get_background_terminal(scope, id).await?;
        let changed = sqlx::query(
            "UPDATE background_terminals
             SET rows=?,cols=?,updated_at=?,revision=revision+1
             WHERE id=? AND status='running'",
        )
        .bind(i64::from(rows))
        .bind(i64::from(cols))
        .bind(Utc::now())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "background terminal is not running".into(),
            ));
        }
        self.get_background_terminal(scope, id).await
    }

    pub async fn finish_background_terminal(
        &self,
        scope: &Scope,
        id: &Id,
        status: BackgroundTerminalStatus,
        exit_code: Option<i32>,
        artifact_id: Option<&Id>,
        failure: Option<&str>,
    ) -> Result<BackgroundTerminalSummary, StorageError> {
        if !matches!(
            status,
            BackgroundTerminalStatus::Exited
                | BackgroundTerminalStatus::Failed
                | BackgroundTerminalStatus::Stopped
                | BackgroundTerminalStatus::Orphaned
        ) {
            return Err(StorageError::InvalidState(
                "background terminal finish status is not terminal".into(),
            ));
        }
        let current = self.get_background_terminal(scope, id).await?;
        if matches!(
            current.status,
            BackgroundTerminalStatus::Exited
                | BackgroundTerminalStatus::Failed
                | BackgroundTerminalStatus::Stopped
                | BackgroundTerminalStatus::Orphaned
        ) {
            return Ok(current);
        }
        let failure = failure
            .map(|value| {
                self.sensitive
                    .seal_text(scope, "background_terminals", id, "failure", value)
            })
            .transpose()?;
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE background_terminals
             SET status=?,exit_code=?,artifact_id=?,failure=?,updated_at=?,completed_at=?,revision=revision+1
             WHERE id=? AND status IN ('starting','running')",
        )
        .bind(background_terminal_status_str(&status))
        .bind(exit_code)
        .bind(artifact_id.map(|value| &value.0))
        .bind(failure)
        .bind(now)
        .bind(now)
        .bind(&id.0)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "background terminal changed concurrently".into(),
            ));
        }
        self.get_background_terminal(scope, id).await
    }

    pub async fn mark_background_terminals_orphaned(&self) -> Result<u64, StorageError> {
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE background_terminals
             SET status='orphaned',updated_at=?,completed_at=?,revision=revision+1
             WHERE status IN ('starting','running')",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected())
    }

    pub async fn create_durable_task(
        &self,
        input: CreateDurableTask,
    ) -> Result<DurableTask, StorageError> {
        if input.kind.trim().is_empty()
            || input.idempotency_key.trim().is_empty()
            || input.max_attempts == 0
            || input.max_runtime_seconds == 0
        {
            return Err(StorageError::InvalidData(
                "kind, idempotency key and positive execution limits are required".into(),
            ));
        }
        let now = Utc::now();
        let id = Id::new("dtask");
        let payload = serde_json::to_string(&input.payload)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let payload = self.sensitive.seal_text(
            &input.scope,
            "durable_tasks",
            &id,
            "payload_json",
            &payload,
        )?;
        let max_cost = i64::try_from(input.max_cost_micros)
            .map_err(|_| StorageError::InvalidData("cost limit is too large".into()))?;
        let max_runner_cost = i64::try_from(input.max_runner_cost_micros)
            .map_err(|_| StorageError::InvalidData("runner cost limit is too large".into()))?;
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query("INSERT INTO durable_tasks (id,organization_id,team_id,actor_id,goal_id,task_id,kind,payload_json,idempotency_key,status,max_attempts,max_runtime_seconds,max_cost_micros,max_runner_cost_micros,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,'queued',?,?,?,?,?,?) ON CONFLICT(organization_id,team_id,idempotency_key) DO NOTHING")
            .bind(&id.0).bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&input.scope.actor_id.0)
            .bind(input.scope.goal_id.as_ref().map(|value| &value.0)).bind(input.scope.task_id.as_ref().map(|value| &value.0))
            .bind(&input.kind).bind(&payload).bind(&input.idempotency_key).bind(i64::from(input.max_attempts))
            .bind(i64::try_from(input.max_runtime_seconds).map_err(|_| StorageError::InvalidData("runtime limit is too large".into()))?)
            .bind(max_cost).bind(max_runner_cost).bind(now).bind(now).execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            let existing = sqlx::query("SELECT * FROM durable_tasks WHERE organization_id=? AND team_id=? AND idempotency_key=?")
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&input.idempotency_key).fetch_one(&mut *tx).await?;
            let task = row_to_durable_task(&existing, &self.sensitive)?;
            if task.kind != input.kind
                || task.payload != input.payload
                || task.max_attempts != input.max_attempts
                || task.max_runtime_seconds != input.max_runtime_seconds
                || task.max_cost_micros != input.max_cost_micros
                || task.max_runner_cost_micros != input.max_runner_cost_micros
            {
                return Err(StorageError::InvalidState(
                    "idempotency key was already used for different work".into(),
                ));
            }
            tx.commit().await?;
            return Ok(task);
        }
        if (max_cost > 0 || max_runner_cost > 0)
            && let Some(budget) = sqlx::query("SELECT id,hard_limit FROM team_budgets WHERE organization_id=? AND team_id=? AND period_start<=? AND period_end>? ORDER BY period_start DESC LIMIT 1")
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(now).bind(now).fetch_optional(&mut *tx).await?
        {
                let budget_id: String = budget.try_get("id")?;
                let hard_limit: bool = budget.try_get("hard_limit")?;
                let reserved = if hard_limit {
                    sqlx::query("UPDATE team_budgets SET model_reserved_micros=model_reserved_micros+?,runner_reserved_micros=runner_reserved_micros+?,updated_at=? WHERE id=? AND model_consumed_micros+model_reserved_micros+?<=model_limit_micros AND runner_consumed_micros+runner_reserved_micros+?<=runner_limit_micros")
                        .bind(max_cost).bind(max_runner_cost).bind(now).bind(&budget_id).bind(max_cost).bind(max_runner_cost).execute(&mut *tx).await?
                } else {
                    sqlx::query("UPDATE team_budgets SET model_reserved_micros=model_reserved_micros+?,runner_reserved_micros=runner_reserved_micros+?,updated_at=? WHERE id=?")
                        .bind(max_cost).bind(max_runner_cost).bind(now).bind(&budget_id).execute(&mut *tx).await?
                };
                if reserved.rows_affected() != 1 {
                    return Err(StorageError::InvalidState("Team model budget cannot reserve task maximum".into()));
                }
                sqlx::query("INSERT INTO team_budget_reservations(durable_task_id,budget_id,reserved_remaining_micros,reserved_runner_remaining_micros,status,created_at,updated_at) VALUES(?,?,?,?,'active',?,?)")
                    .bind(&id.0).bind(budget_id).bind(max_cost).bind(max_runner_cost).bind(now).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        self.get_durable_task(&input.scope, &id).await
    }

    pub async fn get_durable_task(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<DurableTask, StorageError> {
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        ensure_team_scope(&task.scope, scope)?;
        Ok(task)
    }

    pub async fn list_durable_tasks(
        &self,
        scope: &Scope,
    ) -> Result<Vec<DurableTask>, StorageError> {
        let rows = sqlx::query("SELECT * FROM durable_tasks WHERE organization_id=? AND team_id=? ORDER BY created_at DESC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| row_to_durable_task(row, &self.sensitive))
            .collect()
    }

    pub async fn find_paused_durable_task_for_turn(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Option<DurableTask>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM durable_tasks WHERE organization_id=? AND team_id=? AND status='paused'",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        for row in &rows {
            let task = row_to_durable_task(row, &self.sensitive)?;
            if task
                .checkpoint
                .as_ref()
                .and_then(|value| value.get("turn_id"))
                .and_then(serde_json::Value::as_str)
                == Some(turn_id.0.as_str())
            {
                return Ok(Some(task));
            }
        }
        Ok(None)
    }

    pub async fn reconcile_paused_durable_task(
        &self,
        scope: &Scope,
        id: &Id,
        turn_id: &Id,
        succeeded: bool,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let current_row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&current_row, &self.sensitive)?;
        ensure_team_scope(&current.scope, scope)?;
        if current.status != DurableTaskStatus::Paused
            || current
                .checkpoint
                .as_ref()
                .and_then(|value| value.get("turn_id"))
                .and_then(serde_json::Value::as_str)
                != Some(turn_id.0.as_str())
        {
            return Err(StorageError::InvalidState(
                "paused durable task is not linked to this turn".into(),
            ));
        }
        let result = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|value| StorageError::InvalidData(value.to_string()))?;
        let result = result
            .map(|value| {
                self.sensitive
                    .seal_text(&current.scope, "durable_tasks", id, "result_json", &value)
            })
            .transpose()?;
        let error = error
            .map(|value| {
                self.sensitive
                    .seal_text(&current.scope, "durable_tasks", id, "error", value)
            })
            .transpose()?;
        let now = Utc::now();
        let status = if succeeded { "succeeded" } else { "failed" };
        settle_durable_budget(&mut tx, id, 0, 0, true, now).await?;
        let changed = sqlx::query("UPDATE durable_tasks SET status=?,result_json=?,error=?,updated_at=?,completed_at=? WHERE id=? AND status='paused'")
            .bind(status).bind(result).bind(error).bind(now).bind(now).bind(&id.0).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "paused durable task changed during reconciliation".into(),
            ));
        }
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn lease_durable_task(
        &self,
        worker: &str,
        lease_seconds: u64,
    ) -> Result<Option<DurableTask>, StorageError> {
        self.lease_durable_task_kind(worker, lease_seconds, None)
            .await
    }

    pub async fn lease_durable_task_kind(
        &self,
        worker: &str,
        lease_seconds: u64,
        kind: Option<&str>,
    ) -> Result<Option<DurableTask>, StorageError> {
        if worker.trim().is_empty() || lease_seconds == 0 {
            return Err(StorageError::InvalidData(
                "worker and positive lease are required".into(),
            ));
        }
        let now = Utc::now();
        let expires = now
            + chrono::Duration::seconds(
                i64::try_from(lease_seconds)
                    .map_err(|_| StorageError::InvalidData("lease is too large".into()))?,
            );
        let token = Id::new("lease").0;
        let row = sqlx::query("UPDATE durable_tasks SET status='leased',attempt=attempt+1,lease_owner=?,lease_token=?,lease_expires_at=?,started_at=COALESCE(started_at,?),updated_at=? WHERE id=(SELECT d.id FROM durable_tasks d LEFT JOIN durable_task_controls c ON c.organization_id=d.organization_id AND c.team_id=d.team_id LEFT JOIN team_capacity cap ON cap.organization_id=d.organization_id AND cap.team_id=d.team_id WHERE (? IS NULL OR d.kind=?) AND COALESCE(c.enabled,1)=1 AND d.cancel_requested=0 AND d.attempt<d.max_attempts AND d.consumed_cost_micros<=d.max_cost_micros AND (d.status='queued' OR (d.status IN ('leased','running') AND d.lease_expires_at<?)) AND (d.started_at IS NULL OR (julianday(?) - julianday(d.started_at))*86400<d.max_runtime_seconds) AND (cap.team_id IS NULL OR (SELECT COUNT(*) FROM durable_tasks active WHERE active.organization_id=d.organization_id AND active.team_id=d.team_id AND active.status IN ('leased','running') AND active.lease_expires_at>=?) < cap.agent_concurrency) ORDER BY d.created_at LIMIT 1) RETURNING *")
            .bind(worker).bind(&token).bind(expires).bind(now).bind(now).bind(kind).bind(kind).bind(now).bind(now).bind(now).fetch_optional(&self.pool).await?;
        row.as_ref()
            .map(|row| row_to_durable_task(row, &self.sensitive))
            .transpose()
    }

    pub async fn start_durable_task(
        &self,
        id: &Id,
        lease_token: &str,
    ) -> Result<DurableTask, StorageError> {
        self.update_leased_task(
            id,
            lease_token,
            LeasedTaskUpdate {
                status: "running",
                checkpoint: None,
                result: None,
                model_cost_micros: None,
                runner_cost_micros: None,
            },
        )
        .await
    }

    pub async fn renew_durable_task_lease(
        &self,
        id: &Id,
        lease_token: &str,
        lease_seconds: u64,
    ) -> Result<DurableTask, StorageError> {
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&row, &self.sensitive)?;
        let now = Utc::now();
        if current.lease_token.as_deref() != Some(lease_token)
            || !matches!(
                current.status,
                DurableTaskStatus::Leased | DurableTaskStatus::Running
            )
            || current.cancel_requested
            || current.started_at.is_some_and(|started| {
                (now - started).num_seconds()
                    >= i64::try_from(current.max_runtime_seconds).unwrap_or(i64::MAX)
            })
        {
            return Err(StorageError::InvalidState(
                "task lease cannot be renewed".into(),
            ));
        }
        let expires = now
            + chrono::Duration::seconds(
                i64::try_from(lease_seconds)
                    .map_err(|_| StorageError::InvalidData("lease is too large".into()))?,
            );
        let changed = sqlx::query("UPDATE durable_tasks SET lease_expires_at=?,updated_at=? WHERE id=? AND lease_token=? AND cancel_requested=0")
            .bind(expires).bind(now).bind(&id.0).bind(lease_token).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState("task lease changed".into()));
        }
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&self.pool)
            .await?;
        row_to_durable_task(&row, &self.sensitive)
    }

    pub async fn checkpoint_durable_task(
        &self,
        id: &Id,
        lease_token: &str,
        checkpoint: &serde_json::Value,
        consumed_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.update_leased_task(
            id,
            lease_token,
            LeasedTaskUpdate {
                status: "running",
                checkpoint: Some(checkpoint),
                result: None,
                model_cost_micros: Some(consumed_cost_micros),
                runner_cost_micros: None,
            },
        )
        .await
    }

    pub async fn checkpoint_durable_task_costs(
        &self,
        id: &Id,
        lease_token: &str,
        checkpoint: &serde_json::Value,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.update_leased_task(
            id,
            lease_token,
            LeasedTaskUpdate {
                status: "running",
                checkpoint: Some(checkpoint),
                result: None,
                model_cost_micros: Some(consumed_cost_micros),
                runner_cost_micros: Some(consumed_runner_cost_micros),
            },
        )
        .await
    }

    pub async fn complete_durable_task(
        &self,
        id: &Id,
        lease_token: &str,
        result: &serde_json::Value,
        consumed_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.update_leased_task(
            id,
            lease_token,
            LeasedTaskUpdate {
                status: "succeeded",
                checkpoint: None,
                result: Some(result),
                model_cost_micros: Some(consumed_cost_micros),
                runner_cost_micros: None,
            },
        )
        .await
    }

    pub async fn complete_durable_task_costs(
        &self,
        id: &Id,
        lease_token: &str,
        result: &serde_json::Value,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.update_leased_task(
            id,
            lease_token,
            LeasedTaskUpdate {
                status: "succeeded",
                checkpoint: None,
                result: Some(result),
                model_cost_micros: Some(consumed_cost_micros),
                runner_cost_micros: Some(consumed_runner_cost_micros),
            },
        )
        .await
    }

    async fn update_leased_task(
        &self,
        id: &Id,
        lease_token: &str,
        update: LeasedTaskUpdate<'_>,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let current_row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&current_row, &self.sensitive)?;
        if current.lease_token.as_deref() != Some(lease_token)
            || !matches!(
                current.status,
                DurableTaskStatus::Leased | DurableTaskStatus::Running
            )
            || current.cancel_requested
            || current
                .lease_expires_at
                .is_none_or(|expires| expires <= Utc::now())
        {
            return Err(StorageError::InvalidState(
                "active task lease is required".into(),
            ));
        }
        let cost = update
            .model_cost_micros
            .unwrap_or(current.consumed_cost_micros);
        let runner_cost = update
            .runner_cost_micros
            .unwrap_or(current.consumed_runner_cost_micros);
        if cost < current.consumed_cost_micros || cost > current.max_cost_micros {
            return Err(StorageError::InvalidState(
                "task cost budget was exceeded or decreased".into(),
            ));
        }
        if runner_cost < current.consumed_runner_cost_micros
            || runner_cost > current.max_runner_cost_micros
        {
            return Err(StorageError::InvalidState(
                "task runner cost budget was exceeded or decreased".into(),
            ));
        }
        let checkpoint = update
            .checkpoint
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let checkpoint = checkpoint
            .map(|value| {
                self.sensitive.seal_text(
                    &current.scope,
                    "durable_tasks",
                    id,
                    "checkpoint_json",
                    &value,
                )
            })
            .transpose()?;
        let result = update
            .result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let result = result
            .map(|value| {
                self.sensitive
                    .seal_text(&current.scope, "durable_tasks", id, "result_json", &value)
            })
            .transpose()?;
        let now = Utc::now();
        let terminal = update.status == "succeeded";
        settle_durable_budget(
            &mut tx,
            id,
            cost - current.consumed_cost_micros,
            runner_cost - current.consumed_runner_cost_micros,
            terminal,
            now,
        )
        .await?;
        let changed = sqlx::query("UPDATE durable_tasks SET status=?,checkpoint_json=COALESCE(?,checkpoint_json),result_json=COALESCE(?,result_json),consumed_cost_micros=?,consumed_runner_cost_micros=?,lease_owner=CASE WHEN ? THEN NULL ELSE lease_owner END,lease_token=CASE WHEN ? THEN NULL ELSE lease_token END,lease_expires_at=CASE WHEN ? THEN NULL ELSE lease_expires_at END,updated_at=?,completed_at=CASE WHEN ? THEN ? ELSE completed_at END WHERE id=? AND lease_token=?")
            .bind(update.status).bind(checkpoint).bind(result).bind(i64::try_from(cost).map_err(|_| StorageError::InvalidData("cost is too large".into()))?).bind(i64::try_from(runner_cost).map_err(|_| StorageError::InvalidData("runner cost is too large".into()))?)
            .bind(terminal).bind(terminal).bind(terminal).bind(now).bind(terminal).bind(now).bind(&id.0).bind(lease_token).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState("task lease changed".into()));
        }
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn fail_durable_task(
        &self,
        id: &Id,
        lease_token: &str,
        error: &str,
        retryable: bool,
        consumed_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.fail_durable_task_costs_inner(
            id,
            lease_token,
            error,
            retryable,
            consumed_cost_micros,
            None,
        )
        .await
    }

    pub async fn fail_durable_task_costs(
        &self,
        id: &Id,
        lease_token: &str,
        error: &str,
        retryable: bool,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        self.fail_durable_task_costs_inner(
            id,
            lease_token,
            error,
            retryable,
            consumed_cost_micros,
            Some(consumed_runner_cost_micros),
        )
        .await
    }

    async fn fail_durable_task_costs_inner(
        &self,
        id: &Id,
        lease_token: &str,
        error: &str,
        retryable: bool,
        consumed_cost_micros: u64,
        consumed_runner_cost_micros: Option<u64>,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&row, &self.sensitive)?;
        if current.lease_token.as_deref() != Some(lease_token)
            || !matches!(
                current.status,
                DurableTaskStatus::Leased | DurableTaskStatus::Running
            )
        {
            return Err(StorageError::InvalidState(
                "active task lease is required".into(),
            ));
        }
        if consumed_cost_micros < current.consumed_cost_micros
            || consumed_cost_micros > current.max_cost_micros
        {
            return Err(StorageError::InvalidState(
                "task cost budget was exceeded or decreased".into(),
            ));
        }
        let runner_cost =
            consumed_runner_cost_micros.unwrap_or(current.consumed_runner_cost_micros);
        if runner_cost < current.consumed_runner_cost_micros
            || runner_cost > current.max_runner_cost_micros
        {
            return Err(StorageError::InvalidState(
                "task runner cost budget was exceeded or decreased".into(),
            ));
        }
        let retry =
            retryable && current.attempt < current.max_attempts && !current.cancel_requested;
        let status = if retry { "queued" } else { "failed" };
        let error =
            self.sensitive
                .seal_text(&current.scope, "durable_tasks", id, "error", error)?;
        let now = Utc::now();
        settle_durable_budget(
            &mut tx,
            id,
            consumed_cost_micros - current.consumed_cost_micros,
            runner_cost - current.consumed_runner_cost_micros,
            !retry,
            now,
        )
        .await?;
        sqlx::query("UPDATE durable_tasks SET status=?,error=?,consumed_cost_micros=?,consumed_runner_cost_micros=?,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=?,completed_at=? WHERE id=? AND lease_token=?")
            .bind(status).bind(error).bind(i64::try_from(consumed_cost_micros).map_err(|_| StorageError::InvalidData("cost is too large".into()))?).bind(i64::try_from(runner_cost).map_err(|_| StorageError::InvalidData("runner cost is too large".into()))?).bind(now).bind((!retry).then_some(now)).bind(&id.0).bind(lease_token).execute(&mut *tx).await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn set_durable_task_status(
        &self,
        scope: &Scope,
        id: &Id,
        status: DurableTaskStatus,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&row, &self.sensitive)?;
        ensure_team_scope(&current.scope, scope)?;
        let allowed = matches!(
            (&current.status, &status),
            (DurableTaskStatus::Queued, DurableTaskStatus::Paused)
                | (DurableTaskStatus::Leased, DurableTaskStatus::Paused)
                | (DurableTaskStatus::Running, DurableTaskStatus::Paused)
                | (DurableTaskStatus::Paused, DurableTaskStatus::Queued)
                | (_, DurableTaskStatus::Cancelled)
        );
        if !allowed
            || matches!(
                current.status,
                DurableTaskStatus::Succeeded
                    | DurableTaskStatus::Failed
                    | DurableTaskStatus::Cancelled
            )
        {
            return Err(StorageError::InvalidState(
                "invalid durable task transition".into(),
            ));
        }
        let now = Utc::now();
        let terminal = status == DurableTaskStatus::Cancelled;
        if terminal {
            settle_durable_budget(&mut tx, id, 0, 0, true, now).await?;
        }
        sqlx::query("UPDATE durable_tasks SET status=?,cancel_requested=?,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=?,completed_at=? WHERE id=?")
            .bind(durable_task_status_str(&status)).bind(terminal).bind(now).bind(terminal.then_some(now)).bind(&id.0).execute(&mut *tx).await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    /// Requests cancellation without stealing an active worker's lease.
    ///
    /// Queued and paused work can be made terminal immediately. Leased or
    /// running work retains its lease so the owning worker can observe the
    /// request at its next enforcement boundary and atomically persist a
    /// cancellation result. This avoids both dispatching another worker and
    /// losing the final checkpoint/result during cross-process cancellation.
    pub async fn request_durable_task_cancellation(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&row, &self.sensitive)?;
        ensure_team_scope(&current.scope, scope)?;
        if matches!(
            current.status,
            DurableTaskStatus::Succeeded | DurableTaskStatus::Failed | DurableTaskStatus::Cancelled
        ) {
            tx.commit().await?;
            return Ok(current);
        }
        let now = Utc::now();
        let active = matches!(
            current.status,
            DurableTaskStatus::Leased | DurableTaskStatus::Running
        );
        if !active {
            settle_durable_budget(&mut tx, id, 0, 0, true, now).await?;
        }
        let status = if active {
            durable_task_status_str(&current.status)
        } else {
            "cancelled"
        };
        let changed = sqlx::query("UPDATE durable_tasks SET status=?,cancel_requested=1,lease_owner=CASE WHEN ? THEN lease_owner ELSE NULL END,lease_token=CASE WHEN ? THEN lease_token ELSE NULL END,lease_expires_at=CASE WHEN ? THEN lease_expires_at ELSE NULL END,updated_at=?,completed_at=CASE WHEN ? THEN completed_at ELSE ? END WHERE id=? AND cancel_requested=0 AND status=? AND COALESCE(lease_token,'')=COALESCE(?,'')")
            .bind(status)
            .bind(active)
            .bind(active)
            .bind(active)
            .bind(now)
            .bind(active)
            .bind(now)
            .bind(&id.0)
            .bind(durable_task_status_str(&current.status))
            .bind(current.lease_token.as_deref())
            .execute(&mut *tx)
            .await?;
        if changed.rows_affected() == 0 {
            let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
                .bind(&id.0)
                .fetch_one(&mut *tx)
                .await?;
            let task = row_to_durable_task(&row, &self.sensitive)?;
            tx.commit().await?;
            return Ok(task);
        }
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    /// Completes an active cancellation using the current lease and stores the
    /// worker's final content-bounded result in the same transaction.
    pub async fn acknowledge_durable_task_cancellation(
        &self,
        id: &Id,
        lease_token: &str,
        result: &serde_json::Value,
        consumed_cost_micros: u64,
    ) -> Result<DurableTask, StorageError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_durable_task(&row, &self.sensitive)?;
        if current.lease_token.as_deref() != Some(lease_token)
            || !matches!(
                current.status,
                DurableTaskStatus::Leased | DurableTaskStatus::Running
            )
            || !current.cancel_requested
            || current
                .lease_expires_at
                .is_none_or(|expires| expires <= Utc::now())
            || consumed_cost_micros < current.consumed_cost_micros
            || consumed_cost_micros > current.max_cost_micros
        {
            return Err(StorageError::InvalidState(
                "active cancellation lease and valid cost are required".into(),
            ));
        }
        let encoded = serde_json::to_string(result)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            &current.scope,
            "durable_tasks",
            id,
            "result_json",
            &encoded,
        )?;
        let now = Utc::now();
        settle_durable_budget(
            &mut tx,
            id,
            consumed_cost_micros - current.consumed_cost_micros,
            0,
            true,
            now,
        )
        .await?;
        let changed = sqlx::query("UPDATE durable_tasks SET status='cancelled',result_json=?,consumed_cost_micros=?,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=?,completed_at=? WHERE id=? AND lease_token=? AND cancel_requested=1")
            .bind(encoded)
            .bind(i64::try_from(consumed_cost_micros).map_err(|_| StorageError::InvalidData("cost limit is too large".into()))?)
            .bind(now)
            .bind(now)
            .bind(&id.0)
            .bind(lease_token)
            .execute(&mut *tx)
            .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "cancellation lease changed".into(),
            ));
        }
        let row = sqlx::query("SELECT * FROM durable_tasks WHERE id=?")
            .bind(&id.0)
            .fetch_one(&mut *tx)
            .await?;
        let task = row_to_durable_task(&row, &self.sensitive)?;
        tx.commit().await?;
        Ok(task)
    }

    pub async fn set_team_durable_tasks_enabled(
        &self,
        scope: &Scope,
        enabled: bool,
    ) -> Result<(), StorageError> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO durable_task_controls(organization_id,team_id,enabled,updated_at,updated_by) VALUES(?,?,?,?,?) ON CONFLICT(organization_id,team_id) DO UPDATE SET enabled=excluded.enabled,updated_at=excluded.updated_at,updated_by=excluded.updated_by")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(enabled).bind(now).bind(&scope.actor_id.0).execute(&mut *tx).await?;
        if !enabled {
            sqlx::query("UPDATE team_budgets SET model_reserved_micros=model_reserved_micros-COALESCE((SELECT SUM(r.reserved_remaining_micros) FROM team_budget_reservations r JOIN durable_tasks d ON d.id=r.durable_task_id WHERE r.budget_id=team_budgets.id AND r.status='active' AND d.organization_id=? AND d.team_id=?),0),runner_reserved_micros=runner_reserved_micros-COALESCE((SELECT SUM(r.reserved_runner_remaining_micros) FROM team_budget_reservations r JOIN durable_tasks d ON d.id=r.durable_task_id WHERE r.budget_id=team_budgets.id AND r.status='active' AND d.organization_id=? AND d.team_id=?),0),updated_at=? WHERE organization_id=? AND team_id=?")
                .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(now).bind(&scope.organization_id.0).bind(&scope.team_id.0).execute(&mut *tx).await?;
            sqlx::query("UPDATE team_budget_reservations SET reserved_remaining_micros=0,reserved_runner_remaining_micros=0,status='released',updated_at=? WHERE status='active' AND durable_task_id IN (SELECT id FROM durable_tasks WHERE organization_id=? AND team_id=? AND status NOT IN ('succeeded','failed','cancelled'))")
                .bind(now).bind(&scope.organization_id.0).bind(&scope.team_id.0).execute(&mut *tx).await?;
            sqlx::query("UPDATE durable_tasks SET status='cancelled',cancel_requested=1,lease_owner=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=?,completed_at=? WHERE organization_id=? AND team_id=? AND status NOT IN ('succeeded','failed','cancelled')")
            .bind(now).bind(now).bind(&scope.organization_id.0).bind(&scope.team_id.0).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_editor_context(
        &self,
        session_id: &Id,
        input: UpdateEditorContext,
    ) -> Result<EditorContext, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, &input.scope)?;
        if input.protocol_version != opencoding_protocol::IDE_PROTOCOL_VERSION {
            return Err(StorageError::InvalidData(format!(
                "unsupported IDE protocol version {}",
                input.protocol_version
            )));
        }
        if input.workspace_uri != session.workspace_uri {
            return Err(StorageError::ScopeMismatch);
        }
        if input
            .active_document
            .as_ref()
            .and_then(|document| document.text.as_ref())
            .is_some_and(|text| text.len() > 512 * 1024)
            || input
                .selection
                .as_ref()
                .is_some_and(|selection| selection.text.len() > 128 * 1024)
            || input.diagnostics.len() > 500
            || input
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.len() > 8192)
        {
            return Err(StorageError::InvalidData(
                "IDE context exceeds safety limits".into(),
            ));
        }
        let context = EditorContext {
            session_id: session_id.clone(),
            scope: input.scope,
            protocol_version: input.protocol_version,
            client_instance_id: input.client_instance_id,
            workspace_uri: input.workspace_uri,
            active_document: input.active_document,
            selection: input.selection,
            diagnostics: input.diagnostics,
            updated_at: Utc::now(),
        };
        let encoded = serde_json::to_string(&context)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            &context.scope,
            "editor_contexts",
            &context.client_instance_id,
            "context_json",
            &encoded,
        )?;
        sqlx::query("INSERT INTO editor_contexts (session_id,client_instance_id,organization_id,team_id,actor_id,context_json,updated_at) VALUES (?,?,?,?,?,?,?) ON CONFLICT(session_id,client_instance_id) DO UPDATE SET organization_id=excluded.organization_id,team_id=excluded.team_id,actor_id=excluded.actor_id,context_json=excluded.context_json,updated_at=excluded.updated_at")
            .bind(&context.session_id.0).bind(&context.client_instance_id.0).bind(&context.scope.organization_id.0).bind(&context.scope.team_id.0).bind(&context.scope.actor_id.0).bind(encoded).bind(context.updated_at).execute(&self.pool).await?;
        Ok(context)
    }

    pub async fn create_team_goal(&self, input: CreateTeamGoal) -> Result<TeamGoal, StorageError> {
        if input.title.trim().is_empty() || input.outcome_definition.trim().is_empty() {
            return Err(StorageError::InvalidData(
                "goal title and outcome definition are required".into(),
            ));
        }
        let now = Utc::now();
        let goal = TeamGoal {
            id: Id::new("goal"),
            scope: input.scope,
            title: input.title,
            outcome_definition: input.outcome_definition,
            status: GoalStatus::Active,
            target_date: input.target_date,
            created_at: now,
            updated_at: now,
        };
        sqlx::query("INSERT INTO team_goals (id,organization_id,team_id,actor_id,title,outcome_definition,status,target_date,created_at,updated_at) VALUES (?,?,?,?,?,?,'active',?,?,?)")
            .bind(&goal.id.0).bind(&goal.scope.organization_id.0).bind(&goal.scope.team_id.0).bind(&goal.scope.actor_id.0).bind(&goal.title).bind(&goal.outcome_definition).bind(goal.target_date).bind(now).bind(now).execute(&self.pool).await?;
        Ok(goal)
    }

    pub async fn list_team_goals(&self, scope: &Scope) -> Result<Vec<TeamGoal>, StorageError> {
        let rows = sqlx::query("SELECT * FROM team_goals WHERE organization_id = ? AND team_id = ? ORDER BY updated_at DESC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_team_goal).collect()
    }

    pub async fn update_team_goal(
        &self,
        id: &Id,
        input: UpdateTeamGoal,
    ) -> Result<TeamGoal, StorageError> {
        if input.title.trim().is_empty() || input.outcome_definition.trim().is_empty() {
            return Err(StorageError::InvalidData(
                "goal title and outcome definition are required".into(),
            ));
        }
        let current = self.get_team_goal(&input.scope, id).await?;
        let transition_allowed = current.status == input.status
            || matches!(
                (&current.status, &input.status),
                (
                    GoalStatus::Planned,
                    GoalStatus::Active | GoalStatus::Cancelled
                ) | (
                    GoalStatus::Active,
                    GoalStatus::Achieved | GoalStatus::Cancelled
                )
            );
        if !transition_allowed {
            return Err(StorageError::InvalidState(
                "invalid or terminal Team goal transition".into(),
            ));
        }
        if input.status == GoalStatus::Achieved {
            let outcomes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM team_outcomes WHERE organization_id=? AND team_id=? AND goal_id=? AND status='verified'")
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&id.0).fetch_one(&self.pool).await?;
            if outcomes == 0 {
                return Err(StorageError::InvalidState(
                    "goal achievement requires a Verified Team Outcome".into(),
                ));
            }
        }
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE team_goals SET title=?,outcome_definition=?,status=?,target_date=?,actor_id=?,updated_at=? WHERE id=? AND organization_id=? AND team_id=? AND status=?")
            .bind(&input.title).bind(&input.outcome_definition).bind(goal_status_str(&input.status)).bind(input.target_date).bind(&input.scope.actor_id.0).bind(now)
            .bind(&id.0).bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(goal_status_str(&current.status)).execute(&mut *transaction).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "goal changed concurrently".into(),
            ));
        }
        if input.status == GoalStatus::Cancelled {
            sqlx::query("UPDATE team_tasks SET status='cancelled',actor_id=?,updated_at=? WHERE organization_id=? AND team_id=? AND goal_id=? AND status NOT IN ('verified','cancelled')")
                .bind(&input.scope.actor_id.0).bind(now).bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&id.0)
                .execute(&mut *transaction).await?;
            sqlx::query("UPDATE team_goal_runs SET status='cancelled',current_task_id=NULL,actor_id=?,updated_at=?,revision=revision+1 WHERE organization_id=? AND team_id=? AND goal_id=? AND status<>'cancelled'")
                .bind(&input.scope.actor_id.0).bind(now).bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&id.0)
                .execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        self.get_team_goal(&input.scope, id).await
    }

    pub async fn get_or_create_team_goal_run(
        &self,
        goal_id: &Id,
        input: &ContinueTeamGoal,
    ) -> Result<(TeamGoalRun, bool), StorageError> {
        self.get_team_goal(&input.scope, goal_id).await?;
        if input.idempotency_key.trim().is_empty()
            || input.idempotency_key.len() > 100
            || input.max_attempts == 0
            || input.max_runtime_seconds == 0
            || input.max_cost_micros == 0
            || input.max_cost_micros > i64::MAX as u64
        {
            return Err(StorageError::InvalidData(
                "invalid Team Goal run configuration".into(),
            ));
        }
        let now = Utc::now();
        let id = Id::new("goal_run");
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO team_goal_runs
             (id,organization_id,team_id,actor_id,goal_id,idempotency_key,status,workspace_uri,model,max_attempts,max_runtime_seconds,max_cost_micros,current_task_id,created_at,updated_at,revision)
             VALUES (?,?,?,?,?,?,'active',?,?,?,?,?,NULL,?,?,1)",
        )
        .bind(&id.0)
        .bind(&input.scope.organization_id.0)
        .bind(&input.scope.team_id.0)
        .bind(&input.scope.actor_id.0)
        .bind(&goal_id.0)
        .bind(&input.idempotency_key)
        .bind(&input.workspace_uri)
        .bind(&input.model)
        .bind(i64::from(input.max_attempts))
        .bind(
            i64::try_from(input.max_runtime_seconds)
                .map_err(|_| StorageError::InvalidData("Goal run runtime is too large".into()))?,
        )
        .bind(input.max_cost_micros as i64)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1;
        let row = sqlx::query(
            "SELECT * FROM team_goal_runs
             WHERE organization_id=? AND team_id=? AND actor_id=? AND goal_id=? AND idempotency_key=?",
        )
        .bind(&input.scope.organization_id.0)
        .bind(&input.scope.team_id.0)
        .bind(&input.scope.actor_id.0)
        .bind(&goal_id.0)
        .bind(&input.idempotency_key)
        .fetch_one(&self.pool)
        .await?;
        let run = row_to_team_goal_run(&row)?;
        if run.workspace_uri != input.workspace_uri
            || run.model != input.model
            || run.max_attempts != input.max_attempts
            || run.max_runtime_seconds != input.max_runtime_seconds
            || run.max_cost_micros != input.max_cost_micros
        {
            return Err(StorageError::InvalidState(
                "Goal run idempotency key was reused with different limits".into(),
            ));
        }
        Ok((run, inserted))
    }

    pub async fn get_team_goal_run(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<TeamGoalRun, StorageError> {
        let row = sqlx::query("SELECT * FROM team_goal_runs WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let run = row_to_team_goal_run(&row)?;
        ensure_team_scope(&run.scope, scope)?;
        if run.scope.actor_id != scope.actor_id {
            return Err(StorageError::ScopeMismatch);
        }
        Ok(run)
    }

    pub async fn list_team_goal_runs(
        &self,
        scope: &Scope,
        goal_id: Option<&Id>,
    ) -> Result<Vec<TeamGoalRun>, StorageError> {
        let rows = match goal_id {
            Some(goal_id) => {
                sqlx::query(
                    "SELECT * FROM team_goal_runs
                     WHERE organization_id=? AND team_id=? AND actor_id=? AND goal_id=?
                     ORDER BY updated_at DESC",
                )
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0)
                .bind(&scope.actor_id.0)
                .bind(&goal_id.0)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT * FROM team_goal_runs
                     WHERE organization_id=? AND team_id=? AND actor_id=?
                     ORDER BY updated_at DESC",
                )
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0)
                .bind(&scope.actor_id.0)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(row_to_team_goal_run).collect()
    }

    pub async fn update_team_goal_run_status(
        &self,
        scope: &Scope,
        id: &Id,
        status: TeamGoalRunStatus,
        expected_revision: u64,
    ) -> Result<TeamGoalRun, StorageError> {
        let current = self.get_team_goal_run(scope, id).await?;
        if current.revision != expected_revision {
            return Err(StorageError::InvalidState(
                "Goal run changed concurrently".into(),
            ));
        }
        validate_team_goal_run_transition(&current.status, &status)?;
        self.set_team_goal_run_progress(scope, id, status, current.current_task_id)
            .await
    }

    pub async fn set_team_goal_run_progress(
        &self,
        scope: &Scope,
        id: &Id,
        status: TeamGoalRunStatus,
        current_task_id: Option<Id>,
    ) -> Result<TeamGoalRun, StorageError> {
        let current = self.get_team_goal_run(scope, id).await?;
        validate_team_goal_run_transition(&current.status, &status)?;
        let now = Utc::now();
        let revision = i64::try_from(current.revision)
            .map_err(|_| StorageError::InvalidData("Goal run revision is too large".into()))?;
        let changed = sqlx::query(
            "UPDATE team_goal_runs
             SET status=?,current_task_id=?,updated_at=?,revision=revision+1
             WHERE id=? AND organization_id=? AND team_id=? AND actor_id=? AND revision=?",
        )
        .bind(team_goal_run_status_str(&status))
        .bind(current_task_id.as_ref().map(|value| &value.0))
        .bind(now)
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(revision)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Goal run changed concurrently".into(),
            ));
        }
        self.get_team_goal_run(scope, id).await
    }

    pub async fn create_team_task(&self, input: CreateTeamTask) -> Result<TeamTask, StorageError> {
        if input.title.trim().is_empty() || input.source.trim().is_empty() {
            return Err(StorageError::InvalidData(
                "task title and source are required".into(),
            ));
        }
        let now = Utc::now();
        let task = TeamTask {
            id: Id::new("task"),
            scope: input.scope,
            goal_id: input.goal_id,
            source: input.source,
            title: input.title,
            priority: input.priority.clamp(0, 1000),
            assignee_type: input.assignee_type,
            assignee_id: input.assignee_id,
            status: TeamTaskStatus::Ready,
            acceptance_criteria: input.acceptance_criteria,
            required_evidence: input.required_evidence,
            blockers: vec![],
            created_at: now,
            updated_at: now,
        };
        let acceptance = json_text(&task.acceptance_criteria)?;
        let evidence = json_text(&task.required_evidence)?;
        let inserted = if let Some(goal_id) = &task.goal_id {
            sqlx::query("INSERT INTO team_tasks (id,organization_id,team_id,actor_id,goal_id,source,title,priority,assignee_type,assignee_id,status,acceptance_criteria_json,required_evidence_json,blockers_json,created_at,updated_at) SELECT ?,?,?,?,?,?,?,?,?,?,'ready',?,?,?,?,? FROM team_goals WHERE id=? AND organization_id=? AND team_id=? AND status NOT IN ('achieved','cancelled')")
                .bind(&task.id.0).bind(&task.scope.organization_id.0).bind(&task.scope.team_id.0).bind(&task.scope.actor_id.0).bind(&goal_id.0).bind(&task.source).bind(&task.title).bind(task.priority).bind(&task.assignee_type).bind(task.assignee_id.as_ref().map(|id| &id.0))
                .bind(&acceptance).bind(&evidence).bind("[]").bind(now).bind(now)
                .bind(&goal_id.0).bind(&task.scope.organization_id.0).bind(&task.scope.team_id.0)
                .execute(&self.pool).await?
        } else {
            sqlx::query("INSERT INTO team_tasks (id,organization_id,team_id,actor_id,goal_id,source,title,priority,assignee_type,assignee_id,status,acceptance_criteria_json,required_evidence_json,blockers_json,created_at,updated_at) VALUES (?,?,?,?,NULL,?,?,?, ?,?,'ready',?,?,?,?,?)")
                .bind(&task.id.0).bind(&task.scope.organization_id.0).bind(&task.scope.team_id.0).bind(&task.scope.actor_id.0).bind(&task.source).bind(&task.title).bind(task.priority).bind(&task.assignee_type).bind(task.assignee_id.as_ref().map(|id| &id.0))
                .bind(&acceptance).bind(&evidence).bind("[]").bind(now).bind(now)
                .execute(&self.pool).await?
        };
        if inserted.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "cannot add work to a missing or terminal goal".into(),
            ));
        }
        Ok(task)
    }

    pub async fn list_team_tasks(&self, scope: &Scope) -> Result<Vec<TeamTask>, StorageError> {
        let rows = sqlx::query("SELECT * FROM team_tasks WHERE organization_id = ? AND team_id = ? ORDER BY priority DESC, created_at ASC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_team_task).collect()
    }

    pub async fn update_team_task(
        &self,
        id: &Id,
        input: UpdateTeamTask,
    ) -> Result<TeamTask, StorageError> {
        let current = self.get_team_task(&input.scope, id).await?;
        if input.status == TeamTaskStatus::Verified {
            return Err(StorageError::InvalidState(
                "tasks become verified only through a verified outcome".into(),
            ));
        }
        if !valid_task_transition(&current.status, &input.status) {
            return Err(StorageError::InvalidState(format!(
                "invalid task transition {:?} -> {:?}",
                current.status, input.status
            )));
        }
        if input.status == TeamTaskStatus::Blocked && input.blockers.is_empty() {
            return Err(StorageError::InvalidData(
                "blocked tasks require at least one blocker".into(),
            ));
        }
        let now = Utc::now();
        let changed = if input.status == TeamTaskStatus::InProgress {
            sqlx::query("UPDATE team_tasks SET status = ?, assignee_type = ?, assignee_id = ?, blockers_json = ?, actor_id = ?, updated_at = ? WHERE id = ? AND organization_id=? AND team_id=? AND status=? AND (NOT EXISTS(SELECT 1 FROM team_capacity c WHERE c.organization_id=? AND c.team_id=?) OR (SELECT COUNT(*) FROM team_tasks active WHERE active.organization_id=? AND active.team_id=? AND active.status='in_progress' AND active.id<>?) < (SELECT c.wip_limit FROM team_capacity c WHERE c.organization_id=? AND c.team_id=?))")
                .bind(task_status_str(&input.status)).bind(&input.assignee_type).bind(input.assignee_id.as_ref().map(|value| &value.0)).bind(json_text(&input.blockers)?).bind(&input.scope.actor_id.0).bind(now).bind(&id.0)
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0)
                .bind(task_status_str(&current.status))
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0)
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&id.0)
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).execute(&self.pool).await?
        } else {
            sqlx::query("UPDATE team_tasks SET status = ?, assignee_type = ?, assignee_id = ?, blockers_json = ?, actor_id = ?, updated_at = ? WHERE id = ? AND organization_id=? AND team_id=? AND status=?")
                .bind(task_status_str(&input.status)).bind(&input.assignee_type).bind(input.assignee_id.as_ref().map(|value| &value.0)).bind(json_text(&input.blockers)?).bind(&input.scope.actor_id.0).bind(now).bind(&id.0)
                .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(task_status_str(&current.status)).execute(&self.pool).await?
        };
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Team Task changed concurrently or the WIP limit was reached".into(),
            ));
        }
        self.get_team_task(&input.scope, id).await
    }

    pub async fn create_team_knowledge(
        &self,
        input: CreateTeamKnowledgeItem,
    ) -> Result<TeamKnowledgeItem, StorageError> {
        if input.title.trim().is_empty()
            || input.source_uri.trim().is_empty()
            || input.version.trim().is_empty()
            || input.content.len() > 1024 * 1024
            || !valid_knowledge_permission(&input.permission)
        {
            return Err(StorageError::InvalidData(
                "knowledge source, title, version, bounded content and a team or actor permission are required".into(),
            ));
        }
        let now = Utc::now();
        let item = TeamKnowledgeItem {
            id: Id::new("knowledge"),
            owner_team_id: input.scope.team_id.clone(),
            scope: input.scope,
            source_uri: input.source_uri,
            title: input.title,
            content: input.content,
            version: input.version,
            trust_level: input.trust_level,
            permission: input.permission,
            valid_until: input.valid_until,
            created_at: now,
            updated_at: now,
        };
        sqlx::query("INSERT INTO team_knowledge (id,organization_id,team_id,actor_id,source_uri,title,content,version,trust_level,permission,valid_until,owner_team_id,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&item.id.0).bind(&item.scope.organization_id.0).bind(&item.scope.team_id.0).bind(&item.scope.actor_id.0).bind(&item.source_uri).bind(&item.title)
            .bind(self.sensitive.seal_text(&item.scope, "team_knowledge", &item.id, "content", &item.content)?)
            .bind(&item.version).bind(&item.trust_level).bind(&item.permission).bind(item.valid_until).bind(&item.owner_team_id.0).bind(now).bind(now).execute(&self.pool).await?;
        Ok(item)
    }

    pub async fn list_team_knowledge(
        &self,
        scope: &Scope,
        include_expired: bool,
    ) -> Result<Vec<TeamKnowledgeItem>, StorageError> {
        let actor_permission = format!("actor:{}", scope.actor_id.0);
        let rows = if include_expired {
            sqlx::query("SELECT * FROM team_knowledge WHERE organization_id = ? AND team_id = ? AND (permission='team' OR permission=?) ORDER BY updated_at DESC")
                .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&actor_permission).fetch_all(&self.pool).await?
        } else {
            sqlx::query("SELECT * FROM team_knowledge WHERE organization_id = ? AND team_id = ? AND (permission='team' OR permission=?) AND (valid_until IS NULL OR valid_until > ?) ORDER BY updated_at DESC")
                .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&actor_permission).bind(Utc::now()).fetch_all(&self.pool).await?
        };
        rows.iter()
            .map(|row| row_to_team_knowledge(row, &self.sensitive))
            .collect()
    }

    pub async fn get_team_knowledge_for_authorization(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<TeamKnowledgeItem, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM team_knowledge
             WHERE id=? AND organization_id=? AND team_id=?",
        )
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        row_to_team_knowledge(&row, &self.sensitive)
    }

    pub async fn delete_team_knowledge(&self, scope: &Scope, id: &Id) -> Result<(), StorageError> {
        let actor_permission = format!("actor:{}", scope.actor_id.0);
        let deleted = sqlx::query(
            "DELETE FROM team_knowledge
             WHERE id=? AND organization_id=? AND team_id=?
               AND (permission='team' OR permission=?)",
        )
        .bind(&id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(actor_permission)
        .execute(&self.pool)
        .await?;
        if deleted.rows_affected() != 1 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    pub async fn create_team_outcome(
        &self,
        input: CreateTeamOutcome,
    ) -> Result<TeamOutcome, StorageError> {
        let goal = self.get_team_goal(&input.scope, &input.goal_id).await?;
        let task = self.get_team_task(&input.scope, &input.task_id).await?;
        if task.goal_id.as_ref() != Some(&goal.id) || task.status != TeamTaskStatus::Review {
            return Err(StorageError::InvalidState(
                "outcome requires a review-stage task belonging to the goal".into(),
            ));
        }
        let successful = input
            .evidence
            .iter()
            .filter(|evidence| matches!(evidence.result.as_str(), "passed" | "succeeded"))
            .map(|evidence| evidence.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if input.evidence.is_empty()
            || task
                .required_evidence
                .iter()
                .any(|required| !successful.contains(required.as_str()))
        {
            return Err(StorageError::InvalidState(
                "required successful evidence is incomplete".into(),
            ));
        }
        let now = Utc::now();
        let outcome = TeamOutcome {
            id: Id::new("outcome"),
            scope: input.scope,
            goal_id: input.goal_id,
            task_id: input.task_id,
            status: "verified".into(),
            evidence: input.evidence,
            pull_request_url: input.pull_request_url,
            completed_at: now,
        };
        let mut transaction = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE team_tasks SET status = 'verified', updated_at = ?, actor_id = ? WHERE id = ? AND organization_id=? AND team_id=? AND goal_id=? AND status='review'",
        )
        .bind(now)
        .bind(&outcome.scope.actor_id.0)
        .bind(&outcome.task_id.0)
        .bind(&outcome.scope.organization_id.0)
        .bind(&outcome.scope.team_id.0)
        .bind(&outcome.goal_id.0)
        .execute(&mut *transaction)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "review-stage Team Task changed concurrently".into(),
            ));
        }
        sqlx::query("INSERT INTO team_outcomes (id,organization_id,team_id,actor_id,goal_id,task_id,status,evidence_json,pull_request_url,completed_at) VALUES (?,?,?,?,?,?,'verified',?,?,?)")
            .bind(&outcome.id.0).bind(&outcome.scope.organization_id.0).bind(&outcome.scope.team_id.0).bind(&outcome.scope.actor_id.0).bind(&outcome.goal_id.0).bind(&outcome.task_id.0).bind(json_text(&outcome.evidence)?).bind(&outcome.pull_request_url).bind(now).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(outcome)
    }

    pub async fn list_team_outcomes(
        &self,
        scope: &Scope,
    ) -> Result<Vec<TeamOutcome>, StorageError> {
        let rows = sqlx::query("SELECT * FROM team_outcomes WHERE organization_id = ? AND team_id = ? ORDER BY completed_at DESC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_team_outcome).collect()
    }

    pub async fn team_dashboard(
        &self,
        scope: &Scope,
    ) -> Result<TeamDashboardSummary, StorageError> {
        let count = |table: &'static str, status: Option<&'static str>| async move {
            let query = match status {
                Some(_) => format!(
                    "SELECT COUNT(*) FROM {table} WHERE organization_id = ? AND team_id = ? AND status = ?"
                ),
                None => format!(
                    "SELECT COUNT(*) FROM {table} WHERE organization_id = ? AND team_id = ?"
                ),
            };
            let mut query = sqlx::query_scalar::<_, i64>(&query)
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0);
            if let Some(status) = status {
                query = query.bind(status);
            }
            query.fetch_one(&self.pool).await
        };
        let stale: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM team_knowledge WHERE organization_id = ? AND team_id = ? AND valid_until IS NOT NULL AND valid_until <= ?")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(Utc::now()).fetch_one(&self.pool).await?;
        Ok(TeamDashboardSummary {
            team_id: scope.team_id.clone(),
            active_goals: count("team_goals", Some("active")).await? as u64,
            ready_tasks: count("team_tasks", Some("ready")).await? as u64,
            in_progress_tasks: count("team_tasks", Some("in_progress")).await? as u64,
            blocked_tasks: count("team_tasks", Some("blocked")).await? as u64,
            review_tasks: count("team_tasks", Some("review")).await? as u64,
            verified_outcomes: count("team_outcomes", Some("verified")).await? as u64,
            knowledge_items: count("team_knowledge", None).await? as u64,
            stale_knowledge_items: stale as u64,
        })
    }

    pub async fn create_team_ownership(
        &self,
        input: CreateTeamOwnership,
    ) -> Result<TeamOwnership, StorageError> {
        if !matches!(
            input.resource_type.as_str(),
            "repository" | "service" | "environment"
        ) || url::Url::parse(&input.resource_uri).is_err()
        {
            return Err(StorageError::InvalidData(
                "ownership requires a supported resource type and absolute URI".into(),
            ));
        }
        let now = Utc::now();
        let ownership = TeamOwnership {
            id: Id::new("ownership"),
            scope: input.scope,
            resource_type: input.resource_type,
            resource_uri: input.resource_uri,
            service_tier: input.service_tier,
            on_call: input.on_call,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        sqlx::query("INSERT INTO team_ownership(id,organization_id,team_id,actor_id,resource_type,resource_uri,service_tier,on_call,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(&ownership.id.0).bind(&ownership.scope.organization_id.0).bind(&ownership.scope.team_id.0)
            .bind(&ownership.scope.actor_id.0).bind(&ownership.resource_type).bind(&ownership.resource_uri)
            .bind(&ownership.service_tier).bind(&ownership.on_call).bind(now).bind(now).execute(&self.pool).await?;
        Ok(ownership)
    }

    pub async fn list_team_ownership(
        &self,
        scope: &Scope,
    ) -> Result<Vec<TeamOwnership>, StorageError> {
        let rows = sqlx::query("SELECT * FROM team_ownership WHERE organization_id=? AND team_id=? AND archived_at IS NULL ORDER BY resource_type,resource_uri")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_team_ownership).collect()
    }

    pub async fn update_team_ownership(
        &self,
        id: &Id,
        input: UpdateTeamOwnership,
    ) -> Result<TeamOwnership, StorageError> {
        let row = sqlx::query("SELECT * FROM team_ownership WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let current = row_to_team_ownership(&row)?;
        ensure_team_scope(&current.scope, &input.scope)?;
        if current.archived_at.is_some() {
            return Err(StorageError::InvalidState("ownership is archived".into()));
        }
        let target_team = input
            .new_team_id
            .unwrap_or_else(|| input.scope.team_id.clone());
        let now = Utc::now();
        let archived_at = input.archived.then_some(now);
        let changed = sqlx::query("UPDATE team_ownership SET team_id=?,actor_id=?,service_tier=?,on_call=?,updated_at=?,archived_at=? WHERE id=? AND organization_id=? AND team_id=? AND archived_at IS NULL")
            .bind(&target_team.0).bind(&input.scope.actor_id.0).bind(&input.service_tier).bind(&input.on_call).bind(now).bind(archived_at)
            .bind(&id.0).bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "ownership changed concurrently".into(),
            ));
        }
        let row = sqlx::query("SELECT * FROM team_ownership WHERE id=?")
            .bind(&id.0)
            .fetch_one(&self.pool)
            .await?;
        row_to_team_ownership(&row)
    }

    pub async fn upsert_team_capacity(
        &self,
        input: UpdateTeamCapacity,
    ) -> Result<TeamCapacity, StorageError> {
        if !input.human_available_hours.is_finite()
            || input.human_available_hours < 0.0
            || input.wip_limit == 0
            || input.agent_concurrency > 10_000
            || input.wip_limit > 100_000
        {
            return Err(StorageError::InvalidData("invalid Team capacity".into()));
        }
        let now = Utc::now();
        sqlx::query("INSERT INTO team_capacity(organization_id,team_id,actor_id,human_available_hours,agent_concurrency,wip_limit,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(organization_id,team_id) DO UPDATE SET actor_id=excluded.actor_id,human_available_hours=excluded.human_available_hours,agent_concurrency=excluded.agent_concurrency,wip_limit=excluded.wip_limit,updated_at=excluded.updated_at")
            .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(&input.scope.actor_id.0)
            .bind(input.human_available_hours).bind(i64::from(input.agent_concurrency)).bind(i64::from(input.wip_limit)).bind(now).execute(&self.pool).await?;
        self.get_team_capacity(&input.scope).await
    }

    pub async fn get_team_capacity(&self, scope: &Scope) -> Result<TeamCapacity, StorageError> {
        let row = sqlx::query("SELECT * FROM team_capacity WHERE organization_id=? AND team_id=?")
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        row_to_team_capacity(&row)
    }

    pub async fn installed_team_configuration_sequence(
        &self,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<Option<u64>, StorageError> {
        let value = sqlx::query_scalar::<_, i64>("SELECT sequence FROM central_team_configurations WHERE organization_id=? AND team_id=?")
            .bind(&organization_id.0).bind(&team_id.0).fetch_optional(&self.pool).await?;
        value
            .map(|sequence| {
                u64::try_from(sequence).map_err(|_| {
                    StorageError::InvalidData("negative configuration sequence".into())
                })
            })
            .transpose()
    }

    pub async fn apply_verified_team_configuration(
        &self,
        envelope: &SignedTeamConfiguration,
    ) -> Result<TeamCapacity, StorageError> {
        let payload = &envelope.payload;
        let config = &payload.configuration;
        let sequence = i64::try_from(payload.sequence)
            .map_err(|_| StorageError::InvalidData("configuration sequence is too large".into()))?;
        let encoded = serde_json::to_string(envelope)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("INSERT INTO central_team_configurations(organization_id,team_id,sequence,key_id,envelope_json,issued_at,expires_at,applied_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(organization_id,team_id) DO UPDATE SET sequence=excluded.sequence,key_id=excluded.key_id,envelope_json=excluded.envelope_json,issued_at=excluded.issued_at,expires_at=excluded.expires_at,applied_at=excluded.applied_at WHERE excluded.sequence>central_team_configurations.sequence")
            .bind(&payload.organization_id.0).bind(&payload.team_id.0).bind(sequence).bind(&envelope.key_id).bind(encoded)
            .bind(payload.issued_at).bind(payload.expires_at).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "central Team configuration rollback or replay rejected".into(),
            ));
        }
        sqlx::query("INSERT INTO team_capacity(organization_id,team_id,actor_id,human_available_hours,agent_concurrency,wip_limit,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(organization_id,team_id) DO UPDATE SET actor_id=excluded.actor_id,human_available_hours=excluded.human_available_hours,agent_concurrency=excluded.agent_concurrency,wip_limit=excluded.wip_limit,updated_at=excluded.updated_at")
            .bind(&payload.organization_id.0).bind(&payload.team_id.0).bind("central-control-plane")
            .bind(config.human_available_hours).bind(i64::from(config.agent_concurrency)).bind(i64::from(config.wip_limit)).bind(now).execute(&mut *tx).await?;
        let row = sqlx::query("SELECT * FROM team_capacity WHERE organization_id=? AND team_id=?")
            .bind(&payload.organization_id.0)
            .bind(&payload.team_id.0)
            .fetch_one(&mut *tx)
            .await?;
        let capacity = row_to_team_capacity(&row)?;
        tx.commit().await?;
        Ok(capacity)
    }

    pub async fn latest_team_configuration(
        &self,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<Option<SignedTeamConfiguration>, StorageError> {
        let encoded = sqlx::query_scalar::<_, String>("SELECT envelope_json FROM central_team_configurations WHERE organization_id=? AND team_id=?")
            .bind(&organization_id.0).bind(&team_id.0).fetch_optional(&self.pool).await?;
        encoded
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))
    }

    pub async fn installed_team_work_sequence(
        &self,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<Option<u64>, StorageError> {
        sqlx::query_scalar::<_, i64>("SELECT sequence FROM central_team_work_snapshots WHERE organization_id=? AND team_id=?")
            .bind(&organization_id.0)
            .bind(&team_id.0)
            .fetch_optional(&self.pool)
            .await?
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| StorageError::InvalidData("negative work sequence".into()))
            })
            .transpose()
    }

    pub async fn apply_verified_team_work_snapshot(
        &self,
        envelope: &SignedTeamWorkSnapshot,
    ) -> Result<TeamWorkSyncResult, StorageError> {
        let payload = &envelope.payload;
        let sequence = i64::try_from(payload.sequence)
            .map_err(|_| StorageError::InvalidData("work sequence is too large".into()))?;
        let encoded = serde_json::to_string(envelope)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("INSERT INTO central_team_work_snapshots(organization_id,team_id,sequence,key_id,envelope_json,issued_at,expires_at,applied_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(organization_id,team_id) DO UPDATE SET sequence=excluded.sequence,key_id=excluded.key_id,envelope_json=excluded.envelope_json,issued_at=excluded.issued_at,expires_at=excluded.expires_at,applied_at=excluded.applied_at WHERE excluded.sequence>central_team_work_snapshots.sequence")
            .bind(&payload.organization_id.0).bind(&payload.team_id.0).bind(sequence).bind(&envelope.key_id).bind(encoded)
            .bind(payload.issued_at).bind(payload.expires_at).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "central Team work rollback or replay rejected".into(),
            ));
        }

        let local_in_progress: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM team_tasks WHERE organization_id=? AND team_id=? AND central_managed=0 AND status='in_progress'")
            .bind(&payload.organization_id.0).bind(&payload.team_id.0).fetch_one(&mut *tx).await?;
        let central_in_progress = i64::try_from(
            payload
                .tasks
                .iter()
                .filter(|task| task.status == TeamTaskStatus::InProgress)
                .count(),
        )
        .map_err(|_| StorageError::InvalidData("too many central tasks".into()))?;
        let wip_limit = sqlx::query_scalar::<_, i64>(
            "SELECT wip_limit FROM team_capacity WHERE organization_id=? AND team_id=?",
        )
        .bind(&payload.organization_id.0)
        .bind(&payload.team_id.0)
        .fetch_optional(&mut *tx)
        .await?;
        if wip_limit.is_some_and(|limit| local_in_progress + central_in_progress > limit) {
            return Err(StorageError::InvalidState(
                "central Team work snapshot exceeds WIP limit".into(),
            ));
        }

        for goal in &payload.goals {
            let central =
                sqlx::query_scalar::<_, bool>("SELECT central_managed FROM team_goals WHERE id=?")
                    .bind(&goal.id.0)
                    .fetch_optional(&mut *tx)
                    .await?;
            if central == Some(false) {
                return Err(StorageError::InvalidState(format!(
                    "central goal id collides with local goal: {}",
                    goal.id.0
                )));
            }
        }
        for task in &payload.tasks {
            let central =
                sqlx::query_scalar::<_, bool>("SELECT central_managed FROM team_tasks WHERE id=?")
                    .bind(&task.id.0)
                    .fetch_optional(&mut *tx)
                    .await?;
            if central == Some(false) {
                return Err(StorageError::InvalidState(format!(
                    "central task id collides with local task: {}",
                    task.id.0
                )));
            }
        }

        for goal in &payload.goals {
            sqlx::query("INSERT INTO team_goals(id,organization_id,team_id,actor_id,title,outcome_definition,status,target_date,created_at,updated_at,central_managed,central_sequence) VALUES(?,?,?,'central-control-plane',?,?,?,?,?,?,1,?) ON CONFLICT(id) DO UPDATE SET actor_id='central-control-plane',title=excluded.title,outcome_definition=excluded.outcome_definition,status=CASE WHEN team_goals.status='achieved' THEN 'achieved' ELSE excluded.status END,target_date=excluded.target_date,updated_at=excluded.updated_at,central_sequence=excluded.central_sequence WHERE team_goals.central_managed=1 AND team_goals.organization_id=excluded.organization_id AND team_goals.team_id=excluded.team_id")
                .bind(&goal.id.0).bind(&payload.organization_id.0).bind(&payload.team_id.0)
                .bind(&goal.title).bind(&goal.outcome_definition).bind(goal_status_str(&goal.status)).bind(goal.target_date).bind(now).bind(now).bind(sequence)
                .execute(&mut *tx).await?;
        }
        for task in &payload.tasks {
            sqlx::query("INSERT INTO team_tasks(id,organization_id,team_id,actor_id,goal_id,source,title,priority,assignee_type,assignee_id,status,acceptance_criteria_json,required_evidence_json,blockers_json,created_at,updated_at,central_managed,central_sequence) VALUES(?,?,?,'central-control-plane',?,?,?,?,?,?,?,?,?,?,?,?,1,?) ON CONFLICT(id) DO UPDATE SET actor_id='central-control-plane',goal_id=excluded.goal_id,source=excluded.source,title=excluded.title,priority=excluded.priority,assignee_type=excluded.assignee_type,assignee_id=excluded.assignee_id,status=CASE WHEN team_tasks.status='verified' THEN 'verified' ELSE excluded.status END,acceptance_criteria_json=excluded.acceptance_criteria_json,required_evidence_json=excluded.required_evidence_json,blockers_json=excluded.blockers_json,updated_at=excluded.updated_at,central_sequence=excluded.central_sequence WHERE team_tasks.central_managed=1 AND team_tasks.organization_id=excluded.organization_id AND team_tasks.team_id=excluded.team_id")
                .bind(&task.id.0).bind(&payload.organization_id.0).bind(&payload.team_id.0)
                .bind(task.goal_id.as_ref().map(|id| &id.0)).bind(&task.source).bind(&task.title).bind(task.priority.clamp(0, 1000))
                .bind(&task.assignee_type).bind(task.assignee_id.as_ref().map(|id| &id.0)).bind(task_status_str(&task.status))
                .bind(json_text(&task.acceptance_criteria)?).bind(json_text(&task.required_evidence)?).bind(json_text(&task.blockers)?)
                .bind(now).bind(now).bind(sequence).execute(&mut *tx).await?;
        }
        let cancelled_tasks = sqlx::query("UPDATE team_tasks SET status='cancelled',actor_id='central-control-plane',central_sequence=?,updated_at=? WHERE organization_id=? AND team_id=? AND central_managed=1 AND central_sequence<>? AND status NOT IN ('verified','cancelled')")
            .bind(sequence).bind(now).bind(&payload.organization_id.0).bind(&payload.team_id.0).bind(sequence).execute(&mut *tx).await?.rows_affected();
        let cancelled_goals = sqlx::query("UPDATE team_goals SET status='cancelled',actor_id='central-control-plane',central_sequence=?,updated_at=? WHERE organization_id=? AND team_id=? AND central_managed=1 AND central_sequence<>? AND status NOT IN ('achieved','cancelled')")
            .bind(sequence).bind(now).bind(&payload.organization_id.0).bind(&payload.team_id.0).bind(sequence).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(TeamWorkSyncResult {
            sequence: payload.sequence,
            goals: payload.goals.len() as u64,
            tasks: payload.tasks.len() as u64,
            cancelled_goals,
            cancelled_tasks,
        })
    }

    pub async fn latest_team_work_snapshot(
        &self,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<Option<SignedTeamWorkSnapshot>, StorageError> {
        let encoded = sqlx::query_scalar::<_, String>("SELECT envelope_json FROM central_team_work_snapshots WHERE organization_id=? AND team_id=?")
            .bind(&organization_id.0).bind(&team_id.0).fetch_optional(&self.pool).await?;
        encoded
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))
    }

    pub async fn installed_policy_exception_sequence(
        &self,
        exception_id: &Id,
    ) -> Result<Option<u64>, StorageError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT sequence FROM policy_exception_grants WHERE exception_id=?",
        )
        .bind(&exception_id.0)
        .fetch_optional(&self.pool)
        .await?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| StorageError::InvalidData("negative exception sequence".into()))
        })
        .transpose()
    }

    pub async fn apply_verified_policy_exception(
        &self,
        envelope: &SignedPolicyExceptionGrant,
    ) -> Result<CentralPolicyExceptionPayload, StorageError> {
        let payload = &envelope.payload;
        let sequence = i64::try_from(payload.sequence)
            .map_err(|_| StorageError::InvalidData("exception sequence is too large".into()))?;
        let encoded = serde_json::to_string(envelope)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let changed = sqlx::query("INSERT INTO policy_exception_grants(exception_id,organization_id,team_id,sequence,key_id,envelope_json,active,tool,actor_id,workspace_uri,reason,requested_by,approved_by,issued_at,expires_at,applied_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(exception_id) DO UPDATE SET sequence=excluded.sequence,key_id=excluded.key_id,envelope_json=excluded.envelope_json,active=excluded.active,tool=excluded.tool,actor_id=excluded.actor_id,workspace_uri=excluded.workspace_uri,reason=excluded.reason,requested_by=excluded.requested_by,approved_by=excluded.approved_by,issued_at=excluded.issued_at,expires_at=excluded.expires_at,applied_at=excluded.applied_at WHERE excluded.organization_id=policy_exception_grants.organization_id AND excluded.team_id=policy_exception_grants.team_id AND excluded.sequence>policy_exception_grants.sequence")
            .bind(&payload.exception_id.0).bind(&payload.organization_id.0).bind(&payload.team_id.0).bind(sequence).bind(&envelope.key_id).bind(encoded).bind(payload.active).bind(&payload.tool)
            .bind(payload.actor_id.as_ref().map(|value| &value.0)).bind(&payload.workspace_uri).bind(&payload.reason).bind(&payload.requested_by.0).bind(&payload.approved_by.0)
            .bind(payload.issued_at).bind(payload.expires_at).bind(Utc::now()).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "policy exception scope changed, replayed or rolled back".into(),
            ));
        }
        Ok(payload.clone())
    }

    pub async fn matching_policy_exception(
        &self,
        scope: &Scope,
        tool: &str,
        workspace_uri: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<CentralPolicyExceptionPayload>, StorageError> {
        let envelope = sqlx::query_scalar::<_, String>("SELECT envelope_json FROM policy_exception_grants WHERE organization_id=? AND team_id=? AND tool=? AND active=1 AND expires_at>? AND (actor_id IS NULL OR actor_id=?) AND (workspace_uri IS NULL OR workspace_uri=?) ORDER BY sequence DESC LIMIT 1")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(tool).bind(now).bind(&scope.actor_id.0).bind(workspace_uri)
            .fetch_optional(&self.pool).await?;
        envelope
            .map(|value| serde_json::from_str::<SignedPolicyExceptionGrant>(&value))
            .transpose()
            .map(|value| value.map(|envelope| envelope.payload))
            .map_err(|error| StorageError::InvalidData(error.to_string()))
    }

    pub async fn list_policy_exceptions(
        &self,
        scope: &Scope,
    ) -> Result<Vec<SignedPolicyExceptionGrant>, StorageError> {
        let values = sqlx::query_scalar::<_, String>("SELECT envelope_json FROM policy_exception_grants WHERE organization_id=? AND team_id=? ORDER BY applied_at DESC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        values
            .into_iter()
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|error| StorageError::InvalidData(error.to_string()))
            })
            .collect()
    }

    pub async fn create_team_budget(
        &self,
        input: CreateTeamBudget,
    ) -> Result<TeamBudget, StorageError> {
        if input.period_start >= input.period_end
            || input.model_limit_micros > i64::MAX as u64
            || input.runner_limit_micros > i64::MAX as u64
        {
            return Err(StorageError::InvalidData(
                "invalid Team budget period or limit".into(),
            ));
        }
        let overlaps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM team_budgets WHERE organization_id=? AND team_id=? AND period_start < ? AND period_end > ?")
            .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(input.period_end).bind(input.period_start).fetch_one(&self.pool).await?;
        if overlaps != 0 {
            return Err(StorageError::InvalidState(
                "Team budget periods cannot overlap".into(),
            ));
        }
        let now = Utc::now();
        let budget = TeamBudget {
            id: Id::new("budget"),
            scope: input.scope,
            period_start: input.period_start,
            period_end: input.period_end,
            model_limit_micros: input.model_limit_micros,
            runner_limit_micros: input.runner_limit_micros,
            model_consumed_micros: 0,
            runner_consumed_micros: 0,
            model_reserved_micros: 0,
            runner_reserved_micros: 0,
            hard_limit: input.hard_limit,
            created_at: now,
            updated_at: now,
        };
        sqlx::query("INSERT INTO team_budgets(id,organization_id,team_id,actor_id,period_start,period_end,model_limit_micros,runner_limit_micros,hard_limit,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&budget.id.0).bind(&budget.scope.organization_id.0).bind(&budget.scope.team_id.0).bind(&budget.scope.actor_id.0)
            .bind(budget.period_start).bind(budget.period_end).bind(budget.model_limit_micros as i64).bind(budget.runner_limit_micros as i64)
            .bind(budget.hard_limit).bind(now).bind(now).execute(&self.pool).await?;
        Ok(budget)
    }

    pub async fn list_team_budgets(&self, scope: &Scope) -> Result<Vec<TeamBudget>, StorageError> {
        let rows = sqlx::query("SELECT * FROM team_budgets WHERE organization_id=? AND team_id=? ORDER BY period_start DESC")
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).fetch_all(&self.pool).await?;
        rows.iter().map(row_to_team_budget).collect()
    }

    pub async fn consume_team_budget(
        &self,
        input: ConsumeTeamBudget,
    ) -> Result<TeamBudget, StorageError> {
        if input.idempotency_key.trim().is_empty()
            || input.idempotency_key.len() > 200
            || input.model_micros > i64::MAX as u64
            || input.runner_micros > i64::MAX as u64
        {
            return Err(StorageError::InvalidData(
                "invalid budget consumption".into(),
            ));
        }
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT * FROM team_budgets WHERE organization_id=? AND team_id=? AND period_start<=? AND period_end>? ORDER BY period_start DESC LIMIT 1")
            .bind(&input.scope.organization_id.0).bind(&input.scope.team_id.0).bind(now).bind(now)
            .fetch_optional(&mut *tx).await?.ok_or(StorageError::NotFound)?;
        let budget = row_to_team_budget(&row)?;
        if let Some(existing) = sqlx::query("SELECT model_micros,runner_micros FROM team_budget_consumptions WHERE budget_id=? AND idempotency_key=?")
            .bind(&budget.id.0).bind(&input.idempotency_key).fetch_optional(&mut *tx).await? {
            if existing.try_get::<i64,_>("model_micros")? != input.model_micros as i64 || existing.try_get::<i64,_>("runner_micros")? != input.runner_micros as i64 {
                return Err(StorageError::InvalidState("idempotency key reused with different consumption".into()));
            }
            tx.commit().await?;
            return Ok(budget);
        }
        sqlx::query("INSERT INTO team_budget_consumptions(budget_id,idempotency_key,model_micros,runner_micros,created_at) VALUES(?,?,?,?,?)")
            .bind(&budget.id.0).bind(&input.idempotency_key).bind(input.model_micros as i64).bind(input.runner_micros as i64).bind(now).execute(&mut *tx).await?;
        let result = if budget.hard_limit {
            sqlx::query("UPDATE team_budgets SET model_consumed_micros=model_consumed_micros+?,runner_consumed_micros=runner_consumed_micros+?,actor_id=?,updated_at=? WHERE id=? AND model_consumed_micros+model_reserved_micros+?<=model_limit_micros AND runner_consumed_micros+runner_reserved_micros+?<=runner_limit_micros")
                .bind(input.model_micros as i64).bind(input.runner_micros as i64).bind(&input.scope.actor_id.0).bind(now).bind(&budget.id.0)
                .bind(input.model_micros as i64).bind(input.runner_micros as i64).execute(&mut *tx).await?
        } else {
            sqlx::query("UPDATE team_budgets SET model_consumed_micros=model_consumed_micros+?,runner_consumed_micros=runner_consumed_micros+?,actor_id=?,updated_at=? WHERE id=?")
                .bind(input.model_micros as i64).bind(input.runner_micros as i64).bind(&input.scope.actor_id.0).bind(now).bind(&budget.id.0).execute(&mut *tx).await?
        };
        if result.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "Team hard budget limit exceeded".into(),
            ));
        }
        let updated = sqlx::query("SELECT * FROM team_budgets WHERE id=?")
            .bind(&budget.id.0)
            .fetch_one(&mut *tx)
            .await?;
        let updated = row_to_team_budget(&updated)?;
        tx.commit().await?;
        Ok(updated)
    }

    async fn get_team_goal(&self, scope: &Scope, id: &Id) -> Result<TeamGoal, StorageError> {
        let row = sqlx::query("SELECT * FROM team_goals WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let goal = row_to_team_goal(&row)?;
        ensure_team_scope(&goal.scope, scope)?;
        Ok(goal)
    }

    async fn get_team_task(&self, scope: &Scope, id: &Id) -> Result<TeamTask, StorageError> {
        let row = sqlx::query("SELECT * FROM team_tasks WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let task = row_to_team_task(&row)?;
        ensure_team_scope(&task.scope, scope)?;
        Ok(task)
    }

    pub async fn latest_editor_context(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Option<EditorContext>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row = sqlx::query("SELECT client_instance_id,context_json FROM editor_contexts WHERE session_id = ? ORDER BY updated_at DESC LIMIT 1")
            .bind(&session_id.0).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let client_id = Id(row.try_get("client_instance_id")?);
        let encoded: String = row.try_get("context_json")?;
        let plaintext = self.sensitive.open_text(
            scope,
            "editor_contexts",
            &client_id,
            "context_json",
            &encoded,
        )?;
        Ok(Some(serde_json::from_str(&plaintext).map_err(|error| {
            StorageError::InvalidData(error.to_string())
        })?))
    }

    pub async fn append_event(&self, event: &Event, chain_hash: &str) -> Result<u64, StorageError> {
        let payload_item_id = event
            .payload
            .get("item_id")
            .and_then(serde_json::Value::as_str);
        let payload_tool_call_id = event
            .payload
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str);
        let metric = |field: &str| -> Result<Option<i64>, StorageError> {
            event
                .payload
                .get(field)
                .and_then(serde_json::Value::as_u64)
                .map(i64::try_from)
                .transpose()
                .map_err(|_| StorageError::InvalidData(format!("{field} exceeds SQLite range")))
        };
        let payload = serde_json::to_string(&event.payload)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let payload = self.sensitive.seal_text(
            &event.scope,
            "audit_events",
            &event.id,
            "payload_json",
            &payload,
        )?;
        sqlx::query("INSERT INTO audit_events (sequence,id,organization_id,team_id,actor_id,goal_id,task_id,session_id,turn_id,event_type,payload_json,created_at,chain_hash,payload_item_id,payload_tool_call_id,usage_input_units,usage_output_units,usage_model_calls,usage_tool_calls) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(event.sequence as i64).bind(&event.id.0).bind(&event.scope.organization_id.0).bind(&event.scope.team_id.0).bind(&event.scope.actor_id.0)
            .bind(event.scope.goal_id.as_ref().map(|v| &v.0)).bind(event.scope.task_id.as_ref().map(|v| &v.0))
            .bind(event.session_id.as_ref().map(|v| &v.0)).bind(event.turn_id.as_ref().map(|v| &v.0)).bind(&event.kind)
            .bind(payload).bind(event.timestamp).bind(chain_hash)
            .bind(payload_item_id).bind(payload_tool_call_id)
            .bind(metric("input_units")?).bind(metric("output_units")?)
            .bind(metric("model_calls")?).bind(metric("tool_calls")?)
            .execute(&self.pool).await?;
        Ok(event.sequence)
    }

    pub async fn max_event_sequence(&self) -> Result<u64, StorageError> {
        let value: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM audit_events")
            .fetch_one(&self.pool)
            .await?;
        Ok(value as u64)
    }

    pub async fn latest_event_chain_hash(&self) -> Result<Option<String>, StorageError> {
        Ok(
            sqlx::query_scalar(
                "SELECT chain_hash FROM audit_events ORDER BY sequence DESC LIMIT 1",
            )
            .fetch_optional(&self.pool)
            .await?,
        )
    }

    pub async fn audit_chain_records(&self) -> Result<Vec<(Event, String)>, StorageError> {
        let rows = sqlx::query("SELECT * FROM audit_events ORDER BY sequence ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                Ok((
                    row_to_event(row, &self.sensitive)?,
                    row.try_get("chain_hash")?,
                ))
            })
            .collect()
    }

    pub async fn central_audit_export_cursor(
        &self,
        source_id: &Id,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<CentralAuditExportCursor, StorageError> {
        let row = sqlx::query(
            "SELECT organization_id,team_id,source_sequence,local_sequence,chain_head FROM central_audit_export_cursors WHERE source_id=?",
        )
        .bind(&source_id.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(CentralAuditExportCursor {
                source_id: source_id.clone(),
                organization_id: organization_id.clone(),
                team_id: team_id.clone(),
                source_sequence: 0,
                local_sequence: 0,
                chain_head: "0".repeat(64),
            });
        };
        let stored_organization = Id(row.try_get("organization_id")?);
        let stored_team = Id(row.try_get("team_id")?);
        if stored_organization != *organization_id || stored_team != *team_id {
            return Err(StorageError::ScopeMismatch);
        }
        let source_sequence = u64::try_from(row.try_get::<i64, _>("source_sequence")?)
            .map_err(|_| StorageError::InvalidData("negative audit source sequence".into()))?;
        let local_sequence = u64::try_from(row.try_get::<i64, _>("local_sequence")?)
            .map_err(|_| StorageError::InvalidData("negative audit local sequence".into()))?;
        let chain_head: String = row.try_get("chain_head")?;
        if !valid_sha256(&chain_head) {
            return Err(StorageError::InvalidData(
                "central audit cursor chain head is invalid".into(),
            ));
        }
        Ok(CentralAuditExportCursor {
            source_id: source_id.clone(),
            organization_id: stored_organization,
            team_id: stored_team,
            source_sequence,
            local_sequence,
            chain_head,
        })
    }

    pub async fn advance_central_audit_export_cursor(
        &self,
        expected: &CentralAuditExportCursor,
        next: &CentralAuditExportCursor,
    ) -> Result<(), StorageError> {
        if expected.source_id != next.source_id
            || expected.organization_id != next.organization_id
            || expected.team_id != next.team_id
            || next.source_sequence <= expected.source_sequence
            || next.local_sequence <= expected.local_sequence
            || !valid_sha256(&expected.chain_head)
            || !valid_sha256(&next.chain_head)
        {
            return Err(StorageError::InvalidState(
                "central audit cursor transition is invalid".into(),
            ));
        }
        let expected_source = i64::try_from(expected.source_sequence)
            .map_err(|_| StorageError::InvalidState("audit source sequence overflow".into()))?;
        let expected_local = i64::try_from(expected.local_sequence)
            .map_err(|_| StorageError::InvalidState("audit local sequence overflow".into()))?;
        let next_source = i64::try_from(next.source_sequence)
            .map_err(|_| StorageError::InvalidState("audit source sequence overflow".into()))?;
        let next_local = i64::try_from(next.local_sequence)
            .map_err(|_| StorageError::InvalidState("audit local sequence overflow".into()))?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO central_audit_export_cursors(source_id,organization_id,team_id,source_sequence,local_sequence,chain_head,updated_at) VALUES(?,?,?,0,0,?,?) ON CONFLICT(source_id) DO NOTHING")
            .bind(&expected.source_id.0)
            .bind(&expected.organization_id.0)
            .bind(&expected.team_id.0)
            .bind("0".repeat(64))
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        let changed = sqlx::query("UPDATE central_audit_export_cursors SET source_sequence=?,local_sequence=?,chain_head=?,updated_at=? WHERE source_id=? AND organization_id=? AND team_id=? AND source_sequence=? AND local_sequence=? AND chain_head=?")
            .bind(next_source)
            .bind(next_local)
            .bind(&next.chain_head)
            .bind(now)
            .bind(&expected.source_id.0)
            .bind(&expected.organization_id.0)
            .bind(&expected.team_id.0)
            .bind(expected_source)
            .bind(expected_local)
            .bind(&expected.chain_head)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        if changed != 1 {
            transaction.rollback().await?;
            let current = self
                .central_audit_export_cursor(
                    &expected.source_id,
                    &expected.organization_id,
                    &expected.team_id,
                )
                .await?;
            if current == *next {
                return Ok(());
            }
            return Err(StorageError::InvalidState(
                "central audit cursor changed concurrently".into(),
            ));
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn central_audit_pending_export(
        &self,
        source_id: &Id,
        organization_id: &Id,
        team_id: &Id,
    ) -> Result<Option<SignedCentralAuditBatch>, StorageError> {
        let row = sqlx::query("SELECT organization_id,team_id,batch_id,payload_sha256,envelope_json FROM central_audit_export_pending WHERE source_id=?")
            .bind(&source_id.0)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored_organization: String = row.try_get("organization_id")?;
        let stored_team: String = row.try_get("team_id")?;
        if stored_organization != organization_id.0 || stored_team != team_id.0 {
            return Err(StorageError::ScopeMismatch);
        }
        let envelope: SignedCentralAuditBatch =
            serde_json::from_str(&row.try_get::<String, _>("envelope_json")?)
                .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        envelope
            .payload
            .validate()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let digest = envelope
            .payload
            .sha256()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        if envelope.payload.source_id != *source_id
            || envelope.payload.organization_id != *organization_id
            || envelope.payload.team_id != *team_id
            || envelope.payload.batch_id.0 != row.try_get::<String, _>("batch_id")?
            || digest != row.try_get::<String, _>("payload_sha256")?
        {
            return Err(StorageError::InvalidData(
                "pending central audit envelope is corrupted or out of scope".into(),
            ));
        }
        Ok(Some(envelope))
    }

    pub async fn put_central_audit_pending_export(
        &self,
        cursor: &CentralAuditExportCursor,
        envelope: &SignedCentralAuditBatch,
    ) -> Result<SignedCentralAuditBatch, StorageError> {
        let payload = &envelope.payload;
        payload
            .validate()
            .map_err(|error| StorageError::InvalidState(error.to_string()))?;
        if payload.source_id != cursor.source_id
            || payload.organization_id != cursor.organization_id
            || payload.team_id != cursor.team_id
            || payload.previous_sequence != cursor.source_sequence
            || payload.previous_chain_hash != cursor.chain_head
            || payload
                .records
                .first()
                .is_none_or(|record| record.local_sequence <= cursor.local_sequence)
        {
            return Err(StorageError::InvalidState(
                "pending central audit batch does not continue its cursor".into(),
            ));
        }
        let source_sequence = i64::try_from(cursor.source_sequence)
            .map_err(|_| StorageError::InvalidState("audit source sequence overflow".into()))?;
        let local_sequence = i64::try_from(cursor.local_sequence)
            .map_err(|_| StorageError::InvalidState("audit local sequence overflow".into()))?;
        let digest = payload
            .sha256()
            .map_err(|error| StorageError::InvalidState(error.to_string()))?;
        let encoded = serde_json::to_string(envelope)
            .map_err(|error| StorageError::InvalidState(error.to_string()))?;
        let now = Utc::now();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO central_audit_export_cursors(source_id,organization_id,team_id,source_sequence,local_sequence,chain_head,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(source_id) DO NOTHING")
            .bind(&cursor.source_id.0)
            .bind(&cursor.organization_id.0)
            .bind(&cursor.team_id.0)
            .bind(source_sequence)
            .bind(local_sequence)
            .bind(&cursor.chain_head)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        let current = sqlx::query("SELECT organization_id,team_id,source_sequence,local_sequence,chain_head FROM central_audit_export_cursors WHERE source_id=?")
            .bind(&cursor.source_id.0)
            .fetch_one(&mut *transaction)
            .await?;
        if current.try_get::<String, _>("organization_id")? != cursor.organization_id.0
            || current.try_get::<String, _>("team_id")? != cursor.team_id.0
            || current.try_get::<i64, _>("source_sequence")? != source_sequence
            || current.try_get::<i64, _>("local_sequence")? != local_sequence
            || current.try_get::<String, _>("chain_head")? != cursor.chain_head
        {
            return Err(StorageError::InvalidState(
                "central audit cursor changed before pending batch persistence".into(),
            ));
        }
        sqlx::query("INSERT INTO central_audit_export_pending(source_id,organization_id,team_id,batch_id,payload_sha256,envelope_json,created_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(source_id) DO NOTHING")
            .bind(&cursor.source_id.0)
            .bind(&cursor.organization_id.0)
            .bind(&cursor.team_id.0)
            .bind(&payload.batch_id.0)
            .bind(&digest)
            .bind(encoded)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        self.central_audit_pending_export(
            &cursor.source_id,
            &cursor.organization_id,
            &cursor.team_id,
        )
        .await?
        .ok_or_else(|| StorageError::InvalidState("pending central audit batch disappeared".into()))
    }

    pub async fn delete_central_audit_pending_export(
        &self,
        source_id: &Id,
        batch_id: &Id,
        payload_sha256: &str,
    ) -> Result<(), StorageError> {
        let changed = sqlx::query("DELETE FROM central_audit_export_pending WHERE source_id=? AND batch_id=? AND payload_sha256=?")
            .bind(&source_id.0)
            .bind(&batch_id.0)
            .bind(payload_sha256)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed > 1 {
            return Err(StorageError::InvalidData(
                "multiple pending central audit batches were deleted".into(),
            ));
        }
        Ok(())
    }

    pub async fn list_events(
        &self,
        team_id: &Id,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>, StorageError> {
        let rows = sqlx::query("SELECT * FROM audit_events WHERE team_id = ? AND sequence > ? ORDER BY sequence ASC LIMIT ?")
            .bind(&team_id.0).bind(after as i64).bind(limit.min(1000) as i64).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| row_to_event(row, &self.sensitive))
            .collect()
    }

    pub async fn list_team_events(
        &self,
        scope: &Scope,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Event>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM audit_events \
             WHERE organization_id = ? AND team_id = ? AND sequence > ? \
             ORDER BY sequence ASC LIMIT ?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(after as i64)
        .bind(i64::from(limit.clamp(1, 1000)))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row_to_event(row, &self.sensitive))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn list_team_events_filtered(
        &self,
        scope: &Scope,
        after: u64,
        through: Option<u64>,
        limit: u32,
        actor_id: Option<&Id>,
        session_id: Option<&Id>,
        turn_id: Option<&Id>,
        goal_id: Option<&Id>,
        task_id: Option<&Id>,
        kind: Option<&str>,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
    ) -> Result<Vec<Event>, StorageError> {
        let mut query =
            QueryBuilder::<Sqlite>::new("SELECT * FROM audit_events WHERE organization_id=");
        query
            .push_bind(&scope.organization_id.0)
            .push(" AND team_id=")
            .push_bind(&scope.team_id.0)
            .push(" AND sequence>")
            .push_bind(after as i64);
        if let Some(through) = through {
            query.push(" AND sequence<=").push_bind(through as i64);
        }
        if let Some(actor_id) = actor_id {
            query.push(" AND actor_id=").push_bind(&actor_id.0);
        }
        if let Some(session_id) = session_id {
            query.push(" AND session_id=").push_bind(&session_id.0);
        }
        if let Some(turn_id) = turn_id {
            query.push(" AND turn_id=").push_bind(&turn_id.0);
        }
        if let Some(goal_id) = goal_id {
            query.push(" AND goal_id=").push_bind(&goal_id.0);
        }
        if let Some(task_id) = task_id {
            query.push(" AND task_id=").push_bind(&task_id.0);
        }
        if let Some(kind) = kind {
            query.push(" AND event_type=").push_bind(kind);
        }
        if let Some(since) = since {
            query.push(" AND created_at>=").push_bind(since);
        }
        if let Some(until) = until {
            query.push(" AND created_at<=").push_bind(until);
        }
        query
            .push(" ORDER BY sequence ASC LIMIT ")
            .push_bind(i64::from(limit.clamp(1, 1000)));
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| row_to_event(row, &self.sensitive))
            .collect()
    }

    pub async fn list_session_events(
        &self,
        scope: &Scope,
        session_id: &Id,
        limit: u32,
    ) -> Result<Vec<Event>, StorageError> {
        let session = self.get_session(session_id).await?;
        if session.scope.organization_id != scope.organization_id
            || session.scope.team_id != scope.team_id
        {
            return Err(StorageError::ScopeMismatch);
        }
        let rows = sqlx::query(
            "SELECT * FROM audit_events WHERE organization_id=? AND team_id=? AND session_id=? ORDER BY sequence ASC LIMIT ?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&session_id.0)
        .bind(limit.clamp(1, 10_000) as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row_to_event(row, &self.sensitive))
            .collect()
    }

    pub async fn session_usage(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<SessionUsage, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let row = sqlx::query(
            "SELECT
                COALESCE(SUM(usage_input_units), 0) AS input_tokens,
                COALESCE(SUM(usage_output_units), 0) AS output_tokens,
                COALESCE(SUM(usage_model_calls), 0) AS model_calls,
                COALESCE(SUM(usage_tool_calls), 0) AS tool_calls,
                COUNT(DISTINCT turn_id) AS turns
             FROM audit_events
             WHERE organization_id=? AND team_id=? AND session_id=? AND event_type='turn.usage'",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&session_id.0)
        .fetch_one(&self.pool)
        .await?;
        let unsigned = |field: &'static str| -> Result<u64, StorageError> {
            u64::try_from(row.try_get::<i64, _>(field)?)
                .map_err(|_| StorageError::InvalidData(format!("negative {field} usage")))
        };
        let input_tokens = unsigned("input_tokens")?;
        let output_tokens = unsigned("output_tokens")?;
        Ok(SessionUsage {
            input_tokens,
            output_tokens,
            total_tokens: input_tokens.saturating_add(output_tokens),
            model_calls: unsigned("model_calls")?,
            tool_calls: unsigned("tool_calls")?,
            turns: unsigned("turns")?,
        })
    }

    pub async fn latest_event_sequence(&self, team_id: &Id) -> Result<u64, StorageError> {
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM audit_events WHERE team_id = ?",
        )
        .bind(&team_id.0)
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(sequence)
            .map_err(|_| StorageError::InvalidData("negative event sequence".into()))
    }

    pub async fn latest_team_event_sequence(&self, scope: &Scope) -> Result<u64, StorageError> {
        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM audit_events \
             WHERE organization_id = ? AND team_id = ?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(sequence)
            .map_err(|_| StorageError::InvalidData("negative event sequence".into()))
    }

    pub async fn create_tool_call(
        &self,
        request: ToolRequest,
        policy: PolicyResult,
        metadata: ToolPolicyMetadata,
        status: ToolCallStatus,
    ) -> Result<ToolCall, StorageError> {
        let session = self.get_session(&request.session_id).await?;
        ensure_actor_session_scope(&session, &request.scope)?;
        let now = Utc::now();
        let call = ToolCall {
            request,
            policy,
            simulated_policy: metadata.simulated_policy,
            central_policy_applied: metadata.central_policy_applied,
            team_configuration_sequence: metadata.team_configuration_sequence,
            status,
            result: None,
            error: None,
            created_at: now,
            updated_at: now,
        };
        let arguments = serde_json::to_string(&call.request.arguments)
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let arguments = self.sensitive.seal_text(
            &call.request.scope,
            "tool_calls",
            &call.request.id,
            "arguments_json",
            &arguments,
        )?;
        sqlx::query("INSERT INTO tool_calls (id,organization_id,team_id,actor_id,goal_id,task_id,session_id,turn_id,tool,arguments_json,policy_decision,policy_id,policy_version,policy_reason,simulated_policy_json,central_policy_applied,team_configuration_sequence,status,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&call.request.id.0).bind(&call.request.scope.organization_id.0).bind(&call.request.scope.team_id.0).bind(&call.request.scope.actor_id.0)
            .bind(call.request.scope.goal_id.as_ref().map(|v| &v.0)).bind(call.request.scope.task_id.as_ref().map(|v| &v.0)).bind(&call.request.session_id.0).bind(&call.request.turn_id.0)
            .bind(&call.request.tool).bind(arguments)
            .bind(decision_str(&call.policy.decision)).bind(&call.policy.policy_id).bind(&call.policy.policy_version).bind(&call.policy.reason)
            .bind(call.simulated_policy.as_ref().map(json_text).transpose()?).bind(call.central_policy_applied)
            .bind(call.team_configuration_sequence.map(|value| i64::try_from(value).map_err(|_| StorageError::InvalidData("configuration sequence is too large".into()))).transpose()?)
            .bind(tool_status_str(&call.status)).bind(now).bind(now).execute(&self.pool).await?;
        Ok(call)
    }

    pub async fn get_tool_call(&self, id: &Id) -> Result<ToolCall, StorageError> {
        let row = sqlx::query("SELECT * FROM tool_calls WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        row_to_tool_call(&row, &self.sensitive)
    }

    pub async fn list_tool_calls(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<ToolCall>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows =
            sqlx::query("SELECT * FROM tool_calls WHERE session_id = ? ORDER BY created_at ASC")
                .bind(&session_id.0)
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|row| {
                let call = row_to_tool_call(row, &self.sensitive)?;
                ensure_actor_scope(&call.request.scope, scope)?;
                Ok(call)
            })
            .collect()
    }

    pub async fn finish_tool_call(
        &self,
        id: &Id,
        status: ToolCallStatus,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
    ) -> Result<ToolCall, StorageError> {
        let current = self.get_tool_call(id).await?;
        let now = Utc::now();
        let result_json = result
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| StorageError::InvalidData(e.to_string()))?;
        let result_json = result_json
            .map(|value| {
                self.sensitive.seal_text(
                    &current.request.scope,
                    "tool_calls",
                    id,
                    "result_json",
                    &value,
                )
            })
            .transpose()?;
        let error = error
            .map(|value| {
                self.sensitive
                    .seal_text(&current.request.scope, "tool_calls", id, "error", value)
            })
            .transpose()?;
        sqlx::query("UPDATE tool_calls SET status = ?, result_json = ?, error = ?, updated_at = ? WHERE id = ?")
            .bind(tool_status_str(&status)).bind(result_json).bind(error).bind(now).bind(&id.0).execute(&self.pool).await?;
        self.get_tool_call(id).await
    }

    pub async fn create_approval(&self, call: &ToolCall) -> Result<Approval, StorageError> {
        let approval = Approval {
            id: Id::new("apr"),
            tool_call_id: call.request.id.clone(),
            scope: call.request.scope.clone(),
            approval_scope: ApprovalScope::Once,
            status: ApprovalStatus::Pending,
            requested_at: Utc::now(),
            decided_at: None,
            decided_by: None,
        };
        sqlx::query("INSERT INTO approvals (id,tool_call_id,organization_id,team_id,actor_id,goal_id,task_id,approval_scope,status,requested_at) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(&approval.id.0).bind(&approval.tool_call_id.0).bind(&approval.scope.organization_id.0).bind(&approval.scope.team_id.0).bind(&approval.scope.actor_id.0)
            .bind(approval.scope.goal_id.as_ref().map(|v| &v.0)).bind(approval.scope.task_id.as_ref().map(|v| &v.0)).bind("once").bind("pending").bind(approval.requested_at).execute(&self.pool).await?;
        Ok(approval)
    }

    pub async fn create_automatic_approval(
        &self,
        call: &ToolCall,
    ) -> Result<Approval, StorageError> {
        let now = Utc::now();
        let approval = Approval {
            id: Id::new("apr"),
            tool_call_id: call.request.id.clone(),
            scope: call.request.scope.clone(),
            approval_scope: ApprovalScope::Once,
            status: ApprovalStatus::Approved,
            requested_at: now,
            decided_at: Some(now),
            decided_by: Some(call.request.scope.actor_id.clone()),
        };
        sqlx::query("INSERT INTO approvals (id,tool_call_id,organization_id,team_id,actor_id,goal_id,task_id,approval_scope,status,requested_at,decided_at,decided_by) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&approval.id.0).bind(&approval.tool_call_id.0).bind(&approval.scope.organization_id.0).bind(&approval.scope.team_id.0).bind(&approval.scope.actor_id.0)
            .bind(approval.scope.goal_id.as_ref().map(|v| &v.0)).bind(approval.scope.task_id.as_ref().map(|v| &v.0)).bind("once").bind("approved").bind(now).bind(now)
            .bind(&approval.scope.actor_id.0).execute(&self.pool).await?;
        Ok(approval)
    }

    pub async fn get_approval(&self, id: &Id) -> Result<Approval, StorageError> {
        let row = sqlx::query("SELECT * FROM approvals WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        row_to_approval(&row)
    }

    pub async fn list_session_approvals(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<Approval>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT approvals.* FROM approvals \
             INNER JOIN tool_calls ON tool_calls.id = approvals.tool_call_id \
             WHERE tool_calls.session_id = ? ORDER BY approvals.requested_at ASC",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let approval = row_to_approval(row)?;
                ensure_actor_scope(&approval.scope, scope)?;
                Ok(approval)
            })
            .collect()
    }

    pub async fn list_pending_session_approval_calls(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<(Approval, ToolCall)>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT approvals.* FROM approvals \
             INNER JOIN tool_calls ON tool_calls.id=approvals.tool_call_id \
             WHERE tool_calls.session_id=? AND approvals.status='pending' \
             ORDER BY approvals.requested_at,approvals.id LIMIT 1001",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > 1_000 {
            return Err(StorageError::InvalidData(
                "session has more than 1000 pending approvals".into(),
            ));
        }
        let approvals = rows
            .iter()
            .map(|row| {
                let approval = row_to_approval(row)?;
                ensure_actor_scope(&approval.scope, scope)?;
                Ok(approval)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        let tool_ids = approvals
            .iter()
            .map(|approval| approval.tool_call_id.0.clone())
            .collect::<Vec<_>>();
        let tool_rows =
            select_session_rows_by_json_ids(&self.pool, "tool_calls", session_id, &tool_ids)
                .await?;
        let calls = tool_rows
            .iter()
            .map(|row| {
                let call = row_to_tool_call(row, &self.sensitive)?;
                ensure_actor_scope(&call.request.scope, scope)?;
                Ok((call.request.id.0.clone(), call))
            })
            .collect::<Result<std::collections::HashMap<_, _>, StorageError>>()?;
        approvals
            .into_iter()
            .map(|approval| {
                let call = calls
                    .get(&approval.tool_call_id.0)
                    .cloned()
                    .ok_or_else(|| {
                        StorageError::InvalidData(
                            "pending approval references an unavailable tool call".into(),
                        )
                    })?;
                Ok((approval, call))
            })
            .collect()
    }

    pub async fn list_team_approvals(
        &self,
        scope: &Scope,
        actor: Option<&Id>,
        limit: u32,
    ) -> Result<Vec<Approval>, StorageError> {
        let rows = if let Some(actor) = actor {
            sqlx::query(
                "SELECT * FROM approvals \
                 WHERE organization_id = ? AND team_id = ? AND actor_id = ? \
                 ORDER BY requested_at DESC LIMIT ?",
            )
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&actor.0)
            .bind(i64::from(limit.clamp(1, 500)))
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT * FROM approvals \
                 WHERE organization_id = ? AND team_id = ? \
                 ORDER BY requested_at DESC LIMIT ?",
            )
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(i64::from(limit.clamp(1, 500)))
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter()
            .map(|row| {
                let approval = row_to_approval(row)?;
                ensure_team_scope(&approval.scope, scope)?;
                Ok(approval)
            })
            .collect()
    }

    pub async fn resolve_approval(
        &self,
        id: &Id,
        scope: &Scope,
        approved: bool,
        approval_scope: ApprovalScope,
    ) -> Result<Approval, StorageError> {
        let current = self.get_approval(id).await?;
        if current.scope.organization_id != scope.organization_id
            || current.scope.team_id != scope.team_id
        {
            return Err(StorageError::ScopeMismatch);
        }
        if current.status != ApprovalStatus::Pending {
            return Err(StorageError::InvalidState(
                "approval is no longer pending".into(),
            ));
        }
        let now = Utc::now();
        let status = if approved { "approved" } else { "rejected" };
        let result = sqlx::query("UPDATE approvals SET status = ?, approval_scope = ?, decided_at = ?, decided_by = ? WHERE id = ? AND status = 'pending'")
            .bind(status).bind(approval_scope_str(&approval_scope)).bind(now).bind(&scope.actor_id.0).bind(&id.0).execute(&self.pool).await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "approval was resolved concurrently".into(),
            ));
        }
        self.get_approval(id).await
    }

    pub async fn create_question_request(
        &self,
        scope: &Scope,
        request: &QuestionRequest,
    ) -> Result<QuestionRequest, StorageError> {
        let session = self.get_session(&request.session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let turn = self.get_turn(scope, &request.turn_id).await?;
        if turn.session_id != request.session_id
            || request.requested_by != scope.actor_id
            || request.status != QuestionStatus::Pending
            || request.revision != 1
        {
            return Err(StorageError::InvalidState(
                "invalid new question request".into(),
            ));
        }
        let questions = serde_json::to_string(&request.questions)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let questions = self.sensitive.seal_text(
            scope,
            "question_requests",
            &request.id,
            "questions_json",
            &questions,
        )?;
        sqlx::query(
            "INSERT INTO question_requests \
             (id,item_id,organization_id,team_id,actor_id,goal_id,task_id,session_id,turn_id,questions_json,allow_other,status,answers_json,requested_at,expires_at,answered_at,answered_by,revision) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&request.id.0)
        .bind(&request.item_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(scope.goal_id.as_ref().map(|value| &value.0))
        .bind(scope.task_id.as_ref().map(|value| &value.0))
        .bind(&request.session_id.0)
        .bind(&request.turn_id.0)
        .bind(questions)
        .bind(request.allow_other)
        .bind("pending")
        .bind(Option::<String>::None)
        .bind(request.requested_at)
        .bind(request.expires_at)
        .bind(Option::<DateTime<Utc>>::None)
        .bind(Option::<String>::None)
        .bind(1_i64)
        .execute(&self.pool)
        .await?;
        self.get_question_request(scope, &request.id).await
    }

    pub async fn get_question_request(
        &self,
        scope: &Scope,
        id: &Id,
    ) -> Result<QuestionRequest, StorageError> {
        let row = sqlx::query("SELECT * FROM question_requests WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let request = row_to_question_request(&row, &self.sensitive)?;
        let stored_scope = row_scope(&row)?;
        ensure_actor_scope(&stored_scope, scope)?;
        Ok(request)
    }

    pub async fn list_session_question_requests(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<QuestionRequest>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT * FROM question_requests WHERE session_id=? ORDER BY requested_at ASC",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let request = row_to_question_request(row, &self.sensitive)?;
                ensure_actor_scope(&row_scope(row)?, scope)?;
                Ok(request)
            })
            .collect()
    }

    pub async fn list_pending_session_question_requests(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<QuestionRequest>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows = sqlx::query(
            "SELECT * FROM question_requests \
             WHERE session_id=? AND status='pending' \
             ORDER BY requested_at,id LIMIT 1001",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        if rows.len() > 1_000 {
            return Err(StorageError::InvalidData(
                "session has more than 1000 pending questions".into(),
            ));
        }
        rows.iter()
            .map(|row| {
                ensure_actor_scope(&row_scope(row)?, scope)?;
                row_to_question_request(row, &self.sensitive)
            })
            .collect()
    }

    pub async fn list_due_question_requests(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<(Scope, QuestionRequest)>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM question_requests \
             WHERE status='pending' AND expires_at IS NOT NULL AND expires_at<=? \
             ORDER BY expires_at ASC, requested_at ASC",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let scope = row_scope(row)?;
                let request = row_to_question_request(row, &self.sensitive)?;
                Ok((scope, request))
            })
            .collect()
    }

    pub async fn resolve_question_request(
        &self,
        scope: &Scope,
        id: &Id,
        answers: &[QuestionAnswer],
    ) -> Result<QuestionRequest, StorageError> {
        self.resolve_question_request_as(scope, id, answers, &scope.actor_id)
            .await
    }

    pub async fn resolve_question_request_automatically(
        &self,
        scope: &Scope,
        id: &Id,
        answers: &[QuestionAnswer],
    ) -> Result<QuestionRequest, StorageError> {
        self.resolve_question_request_as(scope, id, answers, &Id("system:auto-resolution".into()))
            .await
    }

    async fn resolve_question_request_as(
        &self,
        scope: &Scope,
        id: &Id,
        answers: &[QuestionAnswer],
        answered_by: &Id,
    ) -> Result<QuestionRequest, StorageError> {
        let current = self.get_question_request(scope, id).await?;
        if current.status != QuestionStatus::Pending {
            return Err(StorageError::InvalidState(
                "question is no longer pending".into(),
            ));
        }
        let answers = serde_json::to_string(answers)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let answers =
            self.sensitive
                .seal_text(scope, "question_requests", id, "answers_json", &answers)?;
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE question_requests \
             SET status='answered',answers_json=?,answered_at=?,answered_by=?,revision=revision+1 \
             WHERE id=? AND status='pending' AND revision=?",
        )
        .bind(answers)
        .bind(now)
        .bind(&answered_by.0)
        .bind(&id.0)
        .bind(i64::try_from(current.revision).map_err(|_| {
            StorageError::InvalidData("question revision exceeds SQLite range".into())
        })?)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "question was answered concurrently".into(),
            ));
        }
        self.get_question_request(scope, id).await
    }

    pub async fn create_artifact(
        &self,
        scope: &Scope,
        artifact: &Artifact,
    ) -> Result<Artifact, StorageError> {
        let metadata = &artifact.metadata;
        let session = self.get_session(&metadata.session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let turn = self.get_turn(scope, &metadata.turn_id).await?;
        if turn.session_id != metadata.session_id || metadata.item_id.0.is_empty() {
            return Err(StorageError::InvalidState(
                "artifact ownership does not match its Turn".into(),
            ));
        }
        let title =
            self.sensitive
                .seal_text(scope, "artifacts", &metadata.id, "title", &metadata.title)?;
        let content = serde_json::to_string(&artifact.content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content =
            self.sensitive
                .seal_text(scope, "artifacts", &metadata.id, "content_json", &content)?;
        let byte_length = i64::try_from(metadata.byte_length.unwrap_or(0))
            .map_err(|_| StorageError::InvalidData("artifact is too large".into()))?;
        sqlx::query(
            "INSERT INTO artifacts \
             (id,item_id,organization_id,team_id,actor_id,goal_id,task_id,session_id,turn_id,title,media_type,content_json,byte_length,created_at) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&metadata.id.0)
        .bind(&metadata.item_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(scope.goal_id.as_ref().map(|value| &value.0))
        .bind(scope.task_id.as_ref().map(|value| &value.0))
        .bind(&metadata.session_id.0)
        .bind(&metadata.turn_id.0)
        .bind(title)
        .bind(&metadata.media_type)
        .bind(content)
        .bind(byte_length)
        .bind(metadata.created_at)
        .execute(&self.pool)
        .await?;
        self.get_artifact(scope, &metadata.id).await
    }

    pub async fn get_artifact(&self, scope: &Scope, id: &Id) -> Result<Artifact, StorageError> {
        let row = sqlx::query("SELECT * FROM artifacts WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        ensure_actor_scope(&row_scope(&row)?, scope)?;
        row_to_artifact(&row, &self.sensitive)
    }

    pub async fn list_session_artifacts(
        &self,
        scope: &Scope,
        session_id: &Id,
    ) -> Result<Vec<Artifact>, StorageError> {
        let session = self.get_session(session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        let rows =
            sqlx::query("SELECT * FROM artifacts WHERE session_id=? ORDER BY created_at ASC")
                .bind(&session_id.0)
                .fetch_all(&self.pool)
                .await?;
        rows.iter()
            .map(|row| {
                ensure_actor_scope(&row_scope(row)?, scope)?;
                row_to_artifact(row, &self.sensitive)
            })
            .collect()
    }

    pub async fn list_artifacts(
        &self,
        scope: &Scope,
        session_id: Option<&Id>,
        media_type: Option<&str>,
        before: Option<(&DateTime<Utc>, &Id)>,
        limit: usize,
    ) -> Result<Vec<Artifact>, StorageError> {
        if !(1..=101).contains(&limit) {
            return Err(StorageError::InvalidData(
                "artifact page limit must be between 1 and 101".into(),
            ));
        }
        if let Some(session_id) = session_id {
            let session = self.get_session(session_id).await?;
            ensure_actor_session_scope(&session, scope)?;
        }
        if media_type.is_some_and(|value| value.is_empty() || value.len() > 255) {
            return Err(StorageError::InvalidData(
                "artifact media type filter is invalid".into(),
            ));
        }
        let mut query =
            QueryBuilder::<Sqlite>::new("SELECT * FROM artifacts WHERE organization_id=");
        query
            .push_bind(&scope.organization_id.0)
            .push(" AND team_id=")
            .push_bind(&scope.team_id.0)
            .push(" AND actor_id=")
            .push_bind(&scope.actor_id.0);
        if let Some(session_id) = session_id {
            query.push(" AND session_id=").push_bind(&session_id.0);
        }
        if let Some(media_type) = media_type {
            query.push(" AND media_type=").push_bind(media_type);
        }
        if let Some((created_at, id)) = before {
            query
                .push(" AND (created_at<")
                .push_bind(created_at)
                .push(" OR (created_at=")
                .push_bind(created_at)
                .push(" AND id<")
                .push_bind(&id.0)
                .push("))");
        }
        query
            .push(" ORDER BY created_at DESC,id DESC LIMIT ")
            .push_bind(i64::try_from(limit).map_err(|_| {
                StorageError::InvalidData("artifact page limit exceeds SQLite range".into())
            })?);
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                ensure_actor_scope(&row_scope(row)?, scope)?;
                row_to_artifact(row, &self.sensitive)
            })
            .collect()
    }

    pub async fn install_mcp_server(
        &self,
        scope: &Scope,
        server: &McpServerSpec,
        permissions_sha256: &str,
    ) -> Result<McpInstallation, StorageError> {
        match self.get_mcp_installation(scope, &server.id).await {
            Ok(current) => {
                if current.server == *server
                    && current.permissions_sha256 == permissions_sha256
                    && current.enabled
                {
                    return Ok(current);
                }
                return Err(StorageError::InvalidState(
                    "MCP server id is already installed with a different specification".into(),
                ));
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now();
        let record_id = Id(format!("mcp:{}", server.id));
        let encoded = serde_json::to_string(server)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "mcp_installations",
            &record_id,
            "spec_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO mcp_installations \
             (organization_id,team_id,server_id,actor_id,spec_json,permissions_sha256,enabled,revision,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,1,1,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&server.id)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(permissions_sha256)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_mcp_installation(scope, &server.id).await
    }

    pub async fn get_mcp_installation(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<McpInstallation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM mcp_installations \
             WHERE organization_id=? AND team_id=? AND server_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        row_to_mcp_installation(&row, &self.sensitive)
    }

    pub async fn list_mcp_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<McpInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM mcp_installations \
             WHERE organization_id=? AND team_id=? ORDER BY updated_at DESC,server_id ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| row_to_mcp_installation(row, &self.sensitive))
            .collect()
    }

    pub async fn disable_mcp_server(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE mcp_installations SET enabled=0,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND server_id=? AND enabled=1",
        )
        .bind(Utc::now())
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_mcp_server(
        &self,
        scope: &Scope,
        server_id: &str,
        permissions_sha256: &str,
    ) -> Result<McpInstallation, StorageError> {
        let current = self.get_mcp_installation(scope, server_id).await?;
        if current.permissions_sha256 != permissions_sha256 {
            return Err(StorageError::InvalidState(
                "MCP removal confirmation no longer matches the installed permissions".into(),
            ));
        }
        let changed = sqlx::query(
            "DELETE FROM mcp_installations \
             WHERE organization_id=? AND team_id=? AND server_id=? AND permissions_sha256=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .bind(permissions_sha256)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "MCP installation changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn install_mcp_http_server(
        &self,
        scope: &Scope,
        server: &McpHttpServerSpec,
        permissions_sha256: &str,
    ) -> Result<McpHttpInstallation, StorageError> {
        match self.get_mcp_http_installation(scope, &server.id).await {
            Ok(current) => {
                if current.server == *server
                    && current.permissions_sha256 == permissions_sha256
                    && current.enabled
                {
                    return Ok(current);
                }
                return Err(StorageError::InvalidState(
                    "MCP HTTP server id is already installed with a different specification".into(),
                ));
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now();
        let record_id = Id(format!("mcp-http:{}", server.id));
        let encoded = serde_json::to_string(server)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "mcp_http_installations",
            &record_id,
            "spec_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO mcp_http_installations \
             (organization_id,team_id,server_id,actor_id,spec_json,permissions_sha256,enabled,revision,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,1,1,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&server.id)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(permissions_sha256)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_mcp_http_installation(scope, &server.id).await
    }

    pub async fn get_mcp_http_installation(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<McpHttpInstallation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM mcp_http_installations \
             WHERE organization_id=? AND team_id=? AND server_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let installation = row_to_mcp_http_installation(&row, &self.sensitive)?;
        ensure_team_scope(&installation.scope, scope)?;
        Ok(installation)
    }

    pub async fn list_mcp_http_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<McpHttpInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM mcp_http_installations \
             WHERE organization_id=? AND team_id=? ORDER BY updated_at DESC,server_id ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let installation = row_to_mcp_http_installation(row, &self.sensitive)?;
                ensure_team_scope(&installation.scope, scope)?;
                Ok(installation)
            })
            .collect()
    }

    pub async fn disable_mcp_http_server(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE mcp_http_installations SET enabled=0,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND server_id=? AND enabled=1",
        )
        .bind(Utc::now())
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_mcp_http_server(
        &self,
        scope: &Scope,
        server_id: &str,
        permissions_sha256: &str,
    ) -> Result<McpHttpInstallation, StorageError> {
        let current = self.get_mcp_http_installation(scope, server_id).await?;
        if current.permissions_sha256 != permissions_sha256 {
            return Err(StorageError::InvalidState(
                "MCP HTTP removal confirmation no longer matches the installed permissions".into(),
            ));
        }
        let changed = sqlx::query(
            "DELETE FROM mcp_http_installations \
             WHERE organization_id=? AND team_id=? AND server_id=? AND permissions_sha256=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(server_id)
        .bind(permissions_sha256)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "MCP HTTP installation changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn install_skill(
        &self,
        scope: &Scope,
        skill: &SkillSpec,
        instructions: &str,
        content_sha256: &str,
        permissions_sha256: &str,
    ) -> Result<SkillInstallation, StorageError> {
        match self.get_skill_record(scope, &skill.id).await {
            Ok(current) => {
                if current.installation.skill == *skill
                    && current.instructions == instructions
                    && current.installation.content_sha256 == content_sha256
                    && current.installation.permissions_sha256 == permissions_sha256
                    && current.installation.enabled
                {
                    return Ok(current.installation);
                }
                return Err(StorageError::InvalidState(
                    "Skill id is already installed with different instructions or metadata".into(),
                ));
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now();
        let record_id = Id(format!("skill:{}", skill.id));
        let encoded = serde_json::to_string(skill)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "skill_installations",
            &record_id,
            "spec_json",
            &encoded,
        )?;
        let instructions = self.sensitive.seal_text(
            scope,
            "skill_installations",
            &record_id,
            "instructions",
            instructions,
        )?;
        sqlx::query(
            "INSERT INTO skill_installations \
             (organization_id,team_id,skill_id,actor_id,spec_json,instructions,content_sha256,permissions_sha256,enabled,revision,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,?,?,1,1,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&skill.id)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(instructions)
        .bind(content_sha256)
        .bind(permissions_sha256)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_skill_installation(scope, &skill.id).await
    }

    async fn get_skill_record(
        &self,
        scope: &Scope,
        skill_id: &str,
    ) -> Result<InstalledSkillContext, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM skill_installations \
             WHERE organization_id=? AND team_id=? AND skill_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(skill_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let record = row_to_skill_record(&row, &self.sensitive)?;
        ensure_team_scope(&record.installation.scope, scope)?;
        Ok(record)
    }

    pub async fn get_skill_installation(
        &self,
        scope: &Scope,
        skill_id: &str,
    ) -> Result<SkillInstallation, StorageError> {
        Ok(self.get_skill_record(scope, skill_id).await?.installation)
    }

    pub async fn list_skill_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<SkillInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM skill_installations \
             WHERE organization_id=? AND team_id=? ORDER BY updated_at DESC,skill_id ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let record = row_to_skill_record(row, &self.sensitive)?;
                ensure_team_scope(&record.installation.scope, scope)?;
                Ok(record.installation)
            })
            .collect()
    }

    pub async fn list_enabled_skill_contexts(
        &self,
        scope: &Scope,
    ) -> Result<Vec<InstalledSkillContext>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM skill_installations \
             WHERE organization_id=? AND team_id=? AND enabled=1 \
             ORDER BY updated_at DESC,skill_id ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        let mut contexts = rows
            .iter()
            .map(|row| {
                let record = row_to_skill_record(row, &self.sensitive)?;
                ensure_team_scope(&record.installation.scope, scope)?;
                Ok(record)
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        for plugin in self.list_plugin_installations(scope).await? {
            if !plugin.enabled {
                continue;
            }
            contexts.extend(
                plugin
                    .bundle
                    .skills
                    .into_iter()
                    .map(|asset| InstalledSkillContext {
                        installation: SkillInstallation {
                            scope: scope.clone(),
                            skill: asset.skill,
                            content_sha256: asset.content_sha256,
                            permissions_sha256: asset.permissions_sha256,
                            enabled: true,
                            revision: plugin.revision,
                            created_at: plugin.created_at,
                            updated_at: plugin.updated_at,
                        },
                        instructions: asset.instructions,
                    }),
            );
        }
        Ok(contexts)
    }

    pub async fn set_skill_enabled(
        &self,
        scope: &Scope,
        skill_id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<SkillInstallation, StorageError> {
        let current = self.get_skill_installation(scope, skill_id).await?;
        if current.revision != expected_revision {
            return Err(StorageError::InvalidState(
                "Skill revision changed before the update".into(),
            ));
        }
        if current.enabled == enabled {
            return Ok(current);
        }
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE skill_installations SET enabled=?,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND skill_id=? AND revision=?",
        )
        .bind(enabled)
        .bind(now)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(skill_id)
        .bind(
            i64::try_from(expected_revision)
                .map_err(|_| StorageError::InvalidData("Skill revision is too large".into()))?,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Skill revision changed before the update".into(),
            ));
        }
        self.get_skill_installation(scope, skill_id).await
    }

    pub async fn remove_skill(
        &self,
        scope: &Scope,
        skill_id: &str,
        permissions_sha256: &str,
    ) -> Result<SkillInstallation, StorageError> {
        let current = self.get_skill_installation(scope, skill_id).await?;
        if current.permissions_sha256 != permissions_sha256 {
            return Err(StorageError::InvalidState(
                "Skill removal confirmation no longer matches the installed permissions".into(),
            ));
        }
        let changed = sqlx::query(
            "DELETE FROM skill_installations \
             WHERE organization_id=? AND team_id=? AND skill_id=? AND permissions_sha256=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(skill_id)
        .bind(permissions_sha256)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Skill installation changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn install_hook(
        &self,
        scope: &Scope,
        hook: &HookSpec,
        permissions_sha256: &str,
    ) -> Result<HookInstallation, StorageError> {
        match self.get_hook_installation(scope, &hook.id).await {
            Ok(current) => {
                if current.hook == *hook
                    && current.permissions_sha256 == permissions_sha256
                    && current.enabled
                {
                    return Ok(current);
                }
                return Err(StorageError::InvalidState(
                    "Hook id is already installed with a different specification".into(),
                ));
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now();
        let record_id = Id(format!("hook:{}", hook.id));
        let encoded = serde_json::to_string(hook)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "hook_installations",
            &record_id,
            "spec_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO hook_installations \
             (organization_id,team_id,hook_id,actor_id,spec_json,permissions_sha256,enabled,revision,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,1,1,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&hook.id)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(permissions_sha256)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_hook_installation(scope, &hook.id).await
    }

    pub async fn get_hook_installation(
        &self,
        scope: &Scope,
        hook_id: &str,
    ) -> Result<HookInstallation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM hook_installations \
             WHERE organization_id=? AND team_id=? AND hook_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(hook_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let installation = row_to_hook_installation(&row, &self.sensitive)?;
        ensure_team_scope(&installation.scope, scope)?;
        Ok(installation)
    }

    pub async fn list_hook_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<HookInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM hook_installations \
             WHERE organization_id=? AND team_id=? ORDER BY updated_at DESC,hook_id ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let installation = row_to_hook_installation(row, &self.sensitive)?;
                ensure_team_scope(&installation.scope, scope)?;
                Ok(installation)
            })
            .collect()
    }

    pub async fn disable_hook(&self, scope: &Scope, hook_id: &str) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE hook_installations SET enabled=0,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND hook_id=? AND enabled=1",
        )
        .bind(Utc::now())
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(hook_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_hook(
        &self,
        scope: &Scope,
        hook_id: &str,
        permissions_sha256: &str,
    ) -> Result<HookInstallation, StorageError> {
        let current = self.get_hook_installation(scope, hook_id).await?;
        if current.permissions_sha256 != permissions_sha256 {
            return Err(StorageError::InvalidState(
                "Hook removal confirmation no longer matches the installed permissions".into(),
            ));
        }
        let changed = sqlx::query(
            "DELETE FROM hook_installations \
             WHERE organization_id=? AND team_id=? AND hook_id=? AND permissions_sha256=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(hook_id)
        .bind(permissions_sha256)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Hook installation changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn add_marketplace(
        &self,
        scope: &Scope,
        source: &MarketplaceSource,
        manifest_sha256: &str,
    ) -> Result<MarketplaceInstallation, StorageError> {
        match self.get_marketplace(scope, &source.name).await {
            Ok(current) => {
                if current.source == *source && current.manifest_sha256 == manifest_sha256 {
                    return Ok(current);
                }
                return Err(StorageError::InvalidState(
                    "Marketplace name is already configured with a different source or manifest"
                        .into(),
                ));
            }
            Err(StorageError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let now = Utc::now();
        let record_id = Id(format!("marketplace:{}", source.name));
        let encoded = serde_json::to_string(source)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "marketplace_installations",
            &record_id,
            "source_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO marketplace_installations \
             (organization_id,team_id,marketplace_name,actor_id,source_json,manifest_sha256,revision,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,1,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&source.name)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(manifest_sha256)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_marketplace(scope, &source.name).await
    }

    pub async fn get_marketplace(
        &self,
        scope: &Scope,
        marketplace_name: &str,
    ) -> Result<MarketplaceInstallation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM marketplace_installations \
             WHERE organization_id=? AND team_id=? AND marketplace_name=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(marketplace_name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let installation = row_to_marketplace_installation(&row, &self.sensitive)?;
        ensure_team_scope(&installation.scope, scope)?;
        Ok(installation)
    }

    pub async fn list_marketplaces(
        &self,
        scope: &Scope,
    ) -> Result<Vec<MarketplaceInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM marketplace_installations \
             WHERE organization_id=? AND team_id=? ORDER BY marketplace_name ASC",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let installation = row_to_marketplace_installation(row, &self.sensitive)?;
                ensure_team_scope(&installation.scope, scope)?;
                Ok(installation)
            })
            .collect()
    }

    pub async fn upgrade_marketplace(
        &self,
        scope: &Scope,
        marketplace_name: &str,
        source: &MarketplaceSource,
        manifest_sha256: &str,
    ) -> Result<MarketplaceInstallation, StorageError> {
        if source.name != marketplace_name {
            return Err(StorageError::InvalidData(
                "Marketplace upgrade source does not match its record key".into(),
            ));
        }
        let current = self.get_marketplace(scope, marketplace_name).await?;
        if current.source == *source && current.manifest_sha256 == manifest_sha256 {
            return Ok(current);
        }
        let record_id = Id(format!("marketplace:{marketplace_name}"));
        let encoded = serde_json::to_string(source)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "marketplace_installations",
            &record_id,
            "source_json",
            &encoded,
        )?;
        let changed =
            sqlx::query(
                "UPDATE marketplace_installations \
             SET source_json=?,manifest_sha256=?,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND marketplace_name=? AND revision=?",
            )
            .bind(encoded)
            .bind(manifest_sha256)
            .bind(Utc::now())
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(marketplace_name)
            .bind(i64::try_from(current.revision).map_err(|_| {
                StorageError::InvalidData("Marketplace revision is too large".into())
            })?)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Marketplace changed before upgrade".into(),
            ));
        }
        self.get_marketplace(scope, marketplace_name).await
    }

    pub async fn remove_marketplace(
        &self,
        scope: &Scope,
        marketplace_name: &str,
    ) -> Result<MarketplaceInstallation, StorageError> {
        let current = self.get_marketplace(scope, marketplace_name).await?;
        let installed_plugins: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM plugin_installations \
             WHERE organization_id=? AND team_id=? AND marketplace_name=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(marketplace_name)
        .fetch_one(&self.pool)
        .await?;
        if installed_plugins != 0 {
            return Err(StorageError::InvalidState(
                "Remove installed plugins before removing their Marketplace".into(),
            ));
        }
        let changed =
            sqlx::query(
                "DELETE FROM marketplace_installations \
             WHERE organization_id=? AND team_id=? AND marketplace_name=? AND revision=?",
            )
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(marketplace_name)
            .bind(i64::try_from(current.revision).map_err(|_| {
                StorageError::InvalidData("Marketplace revision is too large".into())
            })?)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Marketplace changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn install_plugin(
        &self,
        scope: &Scope,
        bundle: &PluginBundle,
    ) -> Result<PluginInstallation, StorageError> {
        self.get_marketplace(scope, &bundle.marketplace_name)
            .await?;
        let now = Utc::now();
        let record_id = Id(format!("plugin:{}", bundle.id));
        let encoded = serde_json::to_string(bundle)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "plugin_installations",
            &record_id,
            "bundle_json",
            &encoded,
        )?;
        match self.get_plugin_installation(scope, &bundle.id).await {
            Ok(current) => {
                if current.bundle == *bundle && current.enabled {
                    return Ok(current);
                }
                let changed = sqlx::query(
                    "UPDATE plugin_installations \
                     SET bundle_json=?,permissions_sha256=?,enabled=1,revision=revision+1,updated_at=? \
                     WHERE organization_id=? AND team_id=? AND plugin_id=? AND revision=?",
                )
                .bind(encoded)
                .bind(&bundle.permissions_sha256)
                .bind(now)
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0)
                .bind(&bundle.id)
                .bind(i64::try_from(current.revision).map_err(|_| {
                    StorageError::InvalidData("Plugin revision is too large".into())
                })?)
                .execute(&self.pool)
                .await?
                .rows_affected();
                if changed != 1 {
                    return Err(StorageError::InvalidState(
                        "Plugin changed before installation".into(),
                    ));
                }
            }
            Err(StorageError::NotFound) => {
                sqlx::query(
                    "INSERT INTO plugin_installations \
                     (organization_id,team_id,plugin_id,marketplace_name,plugin_name,actor_id,bundle_json,permissions_sha256,enabled,revision,created_at,updated_at) \
                     VALUES (?,?,?,?,?,?,?,?,1,1,?,?)",
                )
                .bind(&scope.organization_id.0)
                .bind(&scope.team_id.0)
                .bind(&bundle.id)
                .bind(&bundle.marketplace_name)
                .bind(&bundle.name)
                .bind(&scope.actor_id.0)
                .bind(encoded)
                .bind(&bundle.permissions_sha256)
                .bind(now)
                .bind(now)
                .execute(&self.pool)
                .await?;
            }
            Err(error) => return Err(error),
        }
        self.get_plugin_installation(scope, &bundle.id).await
    }

    pub async fn get_plugin_installation(
        &self,
        scope: &Scope,
        plugin_id: &str,
    ) -> Result<PluginInstallation, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM plugin_installations \
             WHERE organization_id=? AND team_id=? AND plugin_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(plugin_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let installation = row_to_plugin_installation(&row, &self.sensitive)?;
        ensure_team_scope(&installation.scope, scope)?;
        Ok(installation)
    }

    pub async fn list_plugin_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<PluginInstallation>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM plugin_installations \
             WHERE organization_id=? AND team_id=? ORDER BY marketplace_name,plugin_name",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let installation = row_to_plugin_installation(row, &self.sensitive)?;
                ensure_team_scope(&installation.scope, scope)?;
                Ok(installation)
            })
            .collect()
    }

    pub async fn set_plugin_enabled(
        &self,
        scope: &Scope,
        plugin_id: &str,
        enabled: bool,
        expected_revision: u64,
    ) -> Result<PluginInstallation, StorageError> {
        let current = self.get_plugin_installation(scope, plugin_id).await?;
        if current.revision != expected_revision {
            return Err(StorageError::InvalidState(
                "Plugin revision changed before the update".into(),
            ));
        }
        if current.enabled == enabled {
            return Ok(current);
        }
        let changed = sqlx::query(
            "UPDATE plugin_installations SET enabled=?,revision=revision+1,updated_at=? \
             WHERE organization_id=? AND team_id=? AND plugin_id=? AND revision=?",
        )
        .bind(enabled)
        .bind(Utc::now())
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(plugin_id)
        .bind(
            i64::try_from(expected_revision)
                .map_err(|_| StorageError::InvalidData("Plugin revision is too large".into()))?,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Plugin revision changed before the update".into(),
            ));
        }
        self.get_plugin_installation(scope, plugin_id).await
    }

    pub async fn remove_plugin(
        &self,
        scope: &Scope,
        plugin_id: &str,
        permissions_sha256: &str,
    ) -> Result<PluginInstallation, StorageError> {
        let current = self.get_plugin_installation(scope, plugin_id).await?;
        if current.bundle.permissions_sha256 != permissions_sha256 {
            return Err(StorageError::InvalidState(
                "Plugin removal confirmation no longer matches the installed permissions".into(),
            ));
        }
        let changed = sqlx::query(
            "DELETE FROM plugin_installations \
             WHERE organization_id=? AND team_id=? AND plugin_id=? AND permissions_sha256=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(plugin_id)
        .bind(permissions_sha256)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "Plugin changed before removal".into(),
            ));
        }
        Ok(current)
    }

    pub async fn list_effective_hook_installations(
        &self,
        scope: &Scope,
    ) -> Result<Vec<HookInstallation>, StorageError> {
        let mut hooks = self
            .list_hook_installations(scope)
            .await?
            .into_iter()
            .filter(|installation| installation.enabled)
            .collect::<Vec<_>>();
        for plugin in self.list_plugin_installations(scope).await? {
            if !plugin.enabled {
                continue;
            }
            hooks.extend(
                plugin
                    .bundle
                    .hooks
                    .into_iter()
                    .map(|hook| HookInstallation {
                        scope: scope.clone(),
                        hook,
                        permissions_sha256: plugin.bundle.permissions_sha256.clone(),
                        enabled: true,
                        revision: plugin.revision,
                        created_at: plugin.created_at,
                        updated_at: plugin.updated_at,
                    }),
            );
        }
        Ok(hooks)
    }

    pub async fn put_mcp_oauth_credential(
        &self,
        scope: &Scope,
        mut credential: McpOAuthCredential,
    ) -> Result<McpOAuthCredential, StorageError> {
        ensure_actor_scope(&credential.scope, scope)?;
        if credential.server_id.is_empty()
            || credential.access_token.is_empty()
            || credential.access_token.len() > 16 * 1024
            || credential
                .refresh_token
                .as_ref()
                .is_some_and(|token| token.is_empty() || token.len() > 16 * 1024)
            || credential.client_id.is_empty()
            || credential.token_endpoint.is_empty()
        {
            return Err(StorageError::InvalidData(
                "MCP OAuth credential is incomplete or exceeds its limits".into(),
            ));
        }
        let now = Utc::now();
        credential.scope = scope.clone();
        credential.updated_at = now;
        if let Ok(current) = self
            .get_mcp_oauth_credential(scope, &credential.server_id)
            .await
        {
            credential.created_at = current.created_at;
        } else {
            credential.created_at = now;
        }
        let record_id = Id(format!("mcp-oauth:{}", credential.server_id));
        let encoded = serde_json::to_string(&credential)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            scope,
            "mcp_oauth_credentials",
            &record_id,
            "credential_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO mcp_oauth_credentials \
             (organization_id,team_id,server_id,actor_id,credential_json,created_at,updated_at) \
             VALUES (?,?,?,?,?,?,?) \
             ON CONFLICT(organization_id,team_id,actor_id,server_id) DO UPDATE SET \
             credential_json=excluded.credential_json,updated_at=excluded.updated_at",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&credential.server_id)
        .bind(&scope.actor_id.0)
        .bind(encoded)
        .bind(credential.created_at)
        .bind(credential.updated_at)
        .execute(&self.pool)
        .await?;
        self.get_mcp_oauth_credential(scope, &credential.server_id)
            .await
    }

    pub async fn get_mcp_oauth_credential(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<McpOAuthCredential, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM mcp_oauth_credentials \
             WHERE organization_id=? AND team_id=? AND actor_id=? AND server_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(server_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::NotFound)?;
        let row_scope = Scope {
            organization_id: Id(row.try_get("organization_id")?),
            team_id: Id(row.try_get("team_id")?),
            actor_id: Id(row.try_get("actor_id")?),
            goal_id: None,
            task_id: None,
        };
        ensure_actor_scope(&row_scope, scope)?;
        let record_id = Id(format!("mcp-oauth:{server_id}"));
        let encoded = self.sensitive.open_text(
            &row_scope,
            "mcp_oauth_credentials",
            &record_id,
            "credential_json",
            &row.try_get::<String, _>("credential_json")?,
        )?;
        let credential: McpOAuthCredential = serde_json::from_str(&encoded)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        if credential.server_id != server_id {
            return Err(StorageError::InvalidData(
                "stored MCP OAuth credential id is inconsistent".into(),
            ));
        }
        Ok(credential)
    }

    pub async fn remove_mcp_oauth_credential(
        &self,
        scope: &Scope,
        server_id: &str,
    ) -> Result<Option<McpOAuthCredential>, StorageError> {
        let current = match self.get_mcp_oauth_credential(scope, server_id).await {
            Ok(current) => current,
            Err(StorageError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        sqlx::query(
            "DELETE FROM mcp_oauth_credentials \
             WHERE organization_id=? AND team_id=? AND actor_id=? AND server_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(server_id)
        .execute(&self.pool)
        .await?;
        Ok(Some(current))
    }

    pub async fn create_pending_mcp_oauth(
        &self,
        pending: &PendingMcpOAuth,
    ) -> Result<(), StorageError> {
        if pending.state.len() < 32
            || pending.state.len() > 256
            || pending.server_id.is_empty()
            || pending.code_verifier.len() < 43
            || pending.code_verifier.len() > 128
            || pending.client_id.is_empty()
            || pending.expires_at <= Utc::now()
        {
            return Err(StorageError::InvalidData(
                "pending MCP OAuth request is invalid".into(),
            ));
        }
        sqlx::query("DELETE FROM mcp_oauth_pending WHERE expires_at<=?")
            .bind(Utc::now())
            .execute(&self.pool)
            .await?;
        let record_id = Id(format!("mcp-oauth-state:{}", pending.state));
        let encoded = serde_json::to_string(pending)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let encoded = self.sensitive.seal_text(
            &pending.scope,
            "mcp_oauth_pending",
            &record_id,
            "pending_json",
            &encoded,
        )?;
        sqlx::query(
            "INSERT INTO mcp_oauth_pending \
             (state,organization_id,team_id,server_id,actor_id,pending_json,expires_at,created_at) \
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(&pending.state)
        .bind(&pending.scope.organization_id.0)
        .bind(&pending.scope.team_id.0)
        .bind(&pending.server_id)
        .bind(&pending.scope.actor_id.0)
        .bind(encoded)
        .bind(pending.expires_at)
        .bind(pending.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn take_pending_mcp_oauth(
        &self,
        state: &str,
    ) -> Result<PendingMcpOAuth, StorageError> {
        let row = sqlx::query("SELECT * FROM mcp_oauth_pending WHERE state=?")
            .bind(state)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StorageError::NotFound)?;
        let scope = Scope {
            organization_id: Id(row.try_get("organization_id")?),
            team_id: Id(row.try_get("team_id")?),
            actor_id: Id(row.try_get("actor_id")?),
            goal_id: None,
            task_id: None,
        };
        let record_id = Id(format!("mcp-oauth-state:{state}"));
        let encoded = self.sensitive.open_text(
            &scope,
            "mcp_oauth_pending",
            &record_id,
            "pending_json",
            &row.try_get::<String, _>("pending_json")?,
        )?;
        let pending: PendingMcpOAuth = serde_json::from_str(&encoded)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let changed = sqlx::query("DELETE FROM mcp_oauth_pending WHERE state=?")
            .bind(state)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed != 1 || pending.state != state || pending.expires_at <= Utc::now() {
            return Err(StorageError::NotFound);
        }
        Ok(pending)
    }

    pub async fn revoke_remote_grant(
        &self,
        scope: &Scope,
        grant_id: &str,
        device_id: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, StorageError> {
        if grant_id.is_empty()
            || grant_id.len() > 256
            || device_id.is_empty()
            || device_id.len() > 256
            || grant_id.chars().any(char::is_control)
            || device_id.chars().any(char::is_control)
            || expires_at <= Utc::now()
        {
            return Err(StorageError::InvalidData(
                "remote grant revocation is invalid".into(),
            ));
        }
        let result = sqlx::query(
            "INSERT INTO remote_grant_revocations \
             (grant_id,organization_id,team_id,actor_id,device_id,expires_at,revoked_at) \
             VALUES (?,?,?,?,?,?,?) ON CONFLICT(grant_id) DO NOTHING",
        )
        .bind(grant_id)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(device_id)
        .bind(expires_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn active_remote_grant_revocations(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, StorageError> {
        sqlx::query("DELETE FROM remote_grant_revocations WHERE expires_at<=?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        let rows = sqlx::query_scalar::<_, String>(
            "SELECT grant_id FROM remote_grant_revocations \
             WHERE expires_at>? ORDER BY grant_id",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

fn sqlite_database_path(url: &str) -> Result<Option<PathBuf>, StorageError> {
    if url == "sqlite::memory:" || url == "sqlite://:memory:" {
        return Ok(None);
    }
    let value = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .ok_or_else(|| StorageError::InvalidData("expected a SQLite database URL".into()))?;
    let value = value.split('?').next().unwrap_or_default();
    if value.is_empty() {
        return Err(StorageError::InvalidData(
            "SQLite database path is empty".into(),
        ));
    }
    Ok(Some(PathBuf::from(value)))
}

fn prepare_private_database_file(path: &Path) -> Result<(), StorageError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        prepare_private_database_directory(parent)?;
    }
    if path.exists() {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(StorageError::InvalidData(
                "SQLite database path must not be a symbolic link".into(),
            ));
        }
        return enforce_private_file(path);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)?;
    enforce_private_file(path)
}

fn prepare_private_database_directory(path: &Path) -> Result<(), StorageError> {
    let existed = path.exists();
    if !existed {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::InvalidData(
            "SQLite state directory must be a regular directory".into(),
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(StorageError::InvalidData(
            "existing SQLite state directory must use mode 0700 or stricter".into(),
        ));
    }
    Ok(())
}

fn enforce_private_file(_path: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn local_settings_scope() -> Scope {
    Scope {
        organization_id: Id("local".into()),
        team_id: Id("local".into()),
        actor_id: Id("daemon".into()),
        goal_id: None,
        task_id: None,
    }
}

fn row_to_session(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<Session, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "active" => SessionStatus::Active,
        "archived" => SessionStatus::Archived,
        "deleted" => SessionStatus::Deleted,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid session status {value}"
            )));
        }
    };
    Ok(Session {
        id: id.clone(),
        scope: scope.clone(),
        workspace_uri: sensitive.open_text(
            &scope,
            "sessions",
            &id,
            "workspace_uri",
            &row.try_get::<String, _>("workspace_uri")?,
        )?,
        title: sensitive.open_text(
            &scope,
            "sessions",
            &id,
            "title",
            &row.try_get::<String, _>("title")?,
        )?,
        model: sensitive.open_text(
            &scope,
            "sessions",
            &id,
            "model",
            &row.try_get::<String, _>("model")?,
        )?,
        status,
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
        updated_at: row.try_get::<DateTime<Utc>, _>("updated_at")?,
    })
}

fn row_to_mcp_installation(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<McpInstallation, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let server_id: String = row.try_get("server_id")?;
    let record_id = Id(format!("mcp:{server_id}"));
    let encoded: String = row.try_get("spec_json")?;
    let encoded = sensitive.open_text(
        &scope,
        "mcp_installations",
        &record_id,
        "spec_json",
        &encoded,
    )?;
    let server: McpServerSpec = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if server.id != server_id {
        return Err(StorageError::InvalidData(
            "stored MCP server id does not match its record key".into(),
        ));
    }
    Ok(McpInstallation {
        scope,
        server,
        permissions_sha256: row.try_get("permissions_sha256")?,
        enabled: row.try_get("enabled")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid MCP revision".into()))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_mcp_http_installation(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<McpHttpInstallation, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let server_id: String = row.try_get("server_id")?;
    let record_id = Id(format!("mcp-http:{server_id}"));
    let encoded: String = row.try_get("spec_json")?;
    let encoded = sensitive.open_text(
        &scope,
        "mcp_http_installations",
        &record_id,
        "spec_json",
        &encoded,
    )?;
    let server: McpHttpServerSpec = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if server.id != server_id {
        return Err(StorageError::InvalidData(
            "stored MCP HTTP server id does not match its record key".into(),
        ));
    }
    Ok(McpHttpInstallation {
        scope,
        server,
        permissions_sha256: row.try_get("permissions_sha256")?,
        enabled: row.try_get("enabled")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid MCP HTTP revision".into()))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_marketplace_installation(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<MarketplaceInstallation, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let marketplace_name: String = row.try_get("marketplace_name")?;
    let record_id = Id(format!("marketplace:{marketplace_name}"));
    let encoded = sensitive.open_text(
        &scope,
        "marketplace_installations",
        &record_id,
        "source_json",
        &row.try_get::<String, _>("source_json")?,
    )?;
    let source: MarketplaceSource = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if source.name != marketplace_name {
        return Err(StorageError::InvalidData(
            "stored Marketplace name does not match its record key".into(),
        ));
    }
    Ok(MarketplaceInstallation {
        scope,
        source,
        manifest_sha256: row.try_get("manifest_sha256")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid Marketplace revision".into()))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_plugin_installation(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<PluginInstallation, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let plugin_id: String = row.try_get("plugin_id")?;
    let record_id = Id(format!("plugin:{plugin_id}"));
    let encoded = sensitive.open_text(
        &scope,
        "plugin_installations",
        &record_id,
        "bundle_json",
        &row.try_get::<String, _>("bundle_json")?,
    )?;
    let bundle: PluginBundle = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if bundle.id != plugin_id
        || bundle.marketplace_name != row.try_get::<String, _>("marketplace_name")?
        || bundle.name != row.try_get::<String, _>("plugin_name")?
        || bundle.permissions_sha256 != row.try_get::<String, _>("permissions_sha256")?
    {
        return Err(StorageError::InvalidData(
            "stored Plugin identity or permission digest is inconsistent".into(),
        ));
    }
    Ok(PluginInstallation {
        scope,
        bundle,
        enabled: row.try_get("enabled")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid Plugin revision".into()))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_skill_record(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<InstalledSkillContext, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let skill_id: String = row.try_get("skill_id")?;
    let record_id = Id(format!("skill:{skill_id}"));
    let encoded: String = row.try_get("spec_json")?;
    let encoded = sensitive.open_text(
        &scope,
        "skill_installations",
        &record_id,
        "spec_json",
        &encoded,
    )?;
    let skill: SkillSpec = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if skill.id != skill_id {
        return Err(StorageError::InvalidData(
            "stored Skill id does not match its record key".into(),
        ));
    }
    let instructions: String = row.try_get("instructions")?;
    let instructions = sensitive.open_text(
        &scope,
        "skill_installations",
        &record_id,
        "instructions",
        &instructions,
    )?;
    let content_sha256: String = row.try_get("content_sha256")?;
    let actual_sha256 = format!("{:x}", Sha256::digest(instructions.as_bytes()));
    if actual_sha256 != content_sha256 {
        return Err(StorageError::InvalidData(
            "stored Skill instructions do not match their digest".into(),
        ));
    }
    Ok(InstalledSkillContext {
        installation: SkillInstallation {
            scope,
            skill,
            content_sha256,
            permissions_sha256: row.try_get("permissions_sha256")?,
            enabled: row.try_get("enabled")?,
            revision: u64::try_from(row.try_get::<i64, _>("revision")?)
                .map_err(|_| StorageError::InvalidData("invalid Skill revision".into()))?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        },
        instructions,
    })
}

fn row_to_hook_installation(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<HookInstallation, StorageError> {
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    let hook_id: String = row.try_get("hook_id")?;
    let record_id = Id(format!("hook:{hook_id}"));
    let encoded: String = row.try_get("spec_json")?;
    let encoded = sensitive.open_text(
        &scope,
        "hook_installations",
        &record_id,
        "spec_json",
        &encoded,
    )?;
    let hook: HookSpec = serde_json::from_str(&encoded)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    if hook.id != hook_id {
        return Err(StorageError::InvalidData(
            "stored Hook id does not match its record key".into(),
        ));
    }
    Ok(HookInstallation {
        scope,
        hook,
        permissions_sha256: row.try_get("permissions_sha256")?,
        enabled: row.try_get("enabled")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid Hook revision".into()))?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_background_terminal(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<BackgroundTerminalSummary, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let unsigned = |column: &str| -> Result<u64, StorageError> {
        u64::try_from(row.try_get::<i64, _>(column)?).map_err(|_| {
            StorageError::InvalidData(format!("negative background terminal value: {column}"))
        })
    };
    Ok(BackgroundTerminalSummary {
        id: id.clone(),
        scope: scope.clone(),
        session_id: Id(row.try_get("session_id")?),
        turn_id: Id(row.try_get("turn_id")?),
        program: sensitive.open_text(
            &scope,
            "background_terminals",
            &id,
            "program",
            &row.try_get::<String, _>("program")?,
        )?,
        argument_count: u32::try_from(unsigned("argument_count")?).map_err(|_| {
            StorageError::InvalidData("terminal argument count is too large".into())
        })?,
        working_directory_uri: sensitive.open_text(
            &scope,
            "background_terminals",
            &id,
            "working_directory_uri",
            &row.try_get::<String, _>("working_directory_uri")?,
        )?,
        status: parse_background_terminal_status(&row.try_get::<String, _>("status")?)?,
        rows: u16::try_from(unsigned("rows")?)
            .map_err(|_| StorageError::InvalidData("terminal rows are too large".into()))?,
        cols: u16::try_from(unsigned("cols")?)
            .map_err(|_| StorageError::InvalidData("terminal columns are too large".into()))?,
        max_runtime_seconds: unsigned("max_runtime_seconds")?,
        output_byte_length: unsigned("output_byte_length")?,
        output_truncated: row.try_get("output_truncated")?,
        exit_code: row.try_get("exit_code")?,
        artifact_id: row.try_get::<Option<String>, _>("artifact_id")?.map(Id),
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
        revision: unsigned("revision")?,
    })
}

fn row_to_session_goal(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<SessionGoal, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_team_scope(row)?;
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "active" => SessionGoalStatus::Active,
        "paused" => SessionGoalStatus::Paused,
        "completed" => SessionGoalStatus::Completed,
        "blocked" => SessionGoalStatus::Blocked,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid session goal status {value}"
            )));
        }
    };
    let unsigned = |column: &str| -> Result<u64, StorageError> {
        u64::try_from(row.try_get::<i64, _>(column)?).map_err(|_| {
            StorageError::InvalidData(format!("negative session goal value: {column}"))
        })
    };
    let token_budget = row
        .try_get::<Option<i64>, _>("token_budget")?
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| StorageError::InvalidData("negative goal token budget".into()))
        })
        .transpose()?;
    Ok(SessionGoal {
        id: id.clone(),
        session_id: Id(row.try_get("session_id")?),
        scope: scope.clone(),
        objective: sensitive.open_text(
            &scope,
            "session_goals",
            &id,
            "objective",
            &row.try_get::<String, _>("objective")?,
        )?,
        status,
        auto_continue: row.try_get("auto_continue")?,
        token_budget,
        input_tokens: unsigned("input_tokens")?,
        output_tokens: unsigned("output_tokens")?,
        continuation_count: u32::try_from(row.try_get::<i64, _>("continuation_count")?)
            .map_err(|_| StorageError::InvalidData("invalid continuation count".into()))?,
        last_turn_id: row.try_get::<Option<String>, _>("last_turn_id")?.map(Id),
        blocked_reason: row
            .try_get::<Option<String>, _>("blocked_reason")?
            .map(|value| {
                sensitive.open_text(&scope, "session_goals", &id, "blocked_reason", &value)
            })
            .transpose()?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
        revision: unsigned("revision")?,
    })
}

fn row_to_side_conversation(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<SideConversation, StorageError> {
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "active" => SideConversationStatus::Active,
        "promoted" => SideConversationStatus::Promoted,
        "closed" => SideConversationStatus::Closed,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid side conversation status {value}"
            )));
        }
    };
    Ok(SideConversation {
        id: Id(row.try_get("id")?),
        scope: Scope {
            organization_id: Id(row.try_get("organization_id")?),
            team_id: Id(row.try_get("team_id")?),
            actor_id: Id(row.try_get("actor_id")?),
            goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
            task_id: row.try_get::<Option<String>, _>("task_id")?.map(Id),
        },
        source_session_id: Id(row.try_get("source_session_id")?),
        session_id: Id(row.try_get("session_id")?),
        source_turn_id: row.try_get::<Option<String>, _>("source_turn_id")?.map(Id),
        status,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_event(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<Event, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
        task_id: row.try_get::<Option<String>, _>("task_id")?.map(Id),
    };
    let payload_text: String = row.try_get("payload_json")?;
    let payload_text =
        sensitive.open_text(&scope, "audit_events", &id, "payload_json", &payload_text)?;
    Ok(Event {
        id,
        sequence: row.try_get::<i64, _>("sequence")? as u64,
        timestamp: row.try_get::<DateTime<Utc>, _>("created_at")?,
        scope,
        session_id: row.try_get::<Option<String>, _>("session_id")?.map(Id),
        turn_id: row.try_get::<Option<String>, _>("turn_id")?.map(Id),
        kind: row.try_get("event_type")?,
        payload: serde_json::from_str(&payload_text)
            .map_err(|e| StorageError::InvalidData(e.to_string()))?,
    })
}

fn row_to_transcript_item_reference(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<TranscriptItemReference, StorageError> {
    let source_kind = match row.try_get::<String, _>("source_kind")?.as_str() {
        "message" => TranscriptItemSourceKind::Message,
        "tool_call" => TranscriptItemSourceKind::ToolCall,
        "approval" => TranscriptItemSourceKind::Approval,
        "question" => TranscriptItemSourceKind::Question,
        "artifact" => TranscriptItemSourceKind::Artifact,
        "plan" => TranscriptItemSourceKind::Plan,
        "context_compaction" => TranscriptItemSourceKind::ContextCompaction,
        "model_reroute" => TranscriptItemSourceKind::ModelReroute,
        "usage" => TranscriptItemSourceKind::Usage,
        "agent_status" => TranscriptItemSourceKind::AgentStatus,
        "hook" => TranscriptItemSourceKind::Hook,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid transcript source kind {value}"
            )));
        }
    };
    Ok(TranscriptItemReference {
        item_id: Id(row.try_get("item_id")?),
        session_id: Id(row.try_get("session_id")?),
        turn_id: Id(row.try_get("turn_id")?),
        source_kind,
        source_id: Id(row.try_get("source_id")?),
        created_at: row.try_get("created_at")?,
    })
}

async fn select_session_rows_by_json_ids(
    pool: &SqlitePool,
    table: &str,
    session_id: &Id,
    ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = match table {
        "messages" => {
            "SELECT * FROM messages WHERE session_id=? \
             AND id IN (SELECT value FROM json_each(?))"
        }
        "tool_calls" => {
            "SELECT * FROM tool_calls WHERE session_id=? \
             AND id IN (SELECT value FROM json_each(?))"
        }
        "question_requests" => {
            "SELECT * FROM question_requests WHERE session_id=? \
             AND id IN (SELECT value FROM json_each(?))"
        }
        "artifacts" => {
            "SELECT * FROM artifacts WHERE session_id=? \
             AND id IN (SELECT value FROM json_each(?))"
        }
        _ => {
            return Err(StorageError::InvalidData(
                "unsupported transcript source table".into(),
            ));
        }
    };
    let ids =
        serde_json::to_string(ids).map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(sql)
        .bind(&session_id.0)
        .bind(ids)
        .fetch_all(pool)
        .await?)
}

async fn select_session_approval_rows_by_json_ids(
    pool: &SqlitePool,
    session_id: &Id,
    ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids =
        serde_json::to_string(ids).map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT approvals.* FROM approvals \
         INNER JOIN tool_calls ON tool_calls.id=approvals.tool_call_id \
         WHERE tool_calls.session_id=? \
         AND approvals.id IN (SELECT value FROM json_each(?))",
    )
    .bind(&session_id.0)
    .bind(ids)
    .fetch_all(pool)
    .await?)
}

async fn select_session_event_rows_by_json_ids(
    pool: &SqlitePool,
    scope: &Scope,
    session_id: &Id,
    ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids =
        serde_json::to_string(ids).map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT * FROM audit_events \
         WHERE organization_id=? AND team_id=? AND session_id=? \
         AND id IN (SELECT value FROM json_each(?))",
    )
    .bind(&scope.organization_id.0)
    .bind(&scope.team_id.0)
    .bind(&session_id.0)
    .bind(ids)
    .fetch_all(pool)
    .await?)
}

async fn select_latest_plan_event_rows(
    pool: &SqlitePool,
    scope: &Scope,
    session_id: &Id,
    item_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let item_ids = serde_json::to_string(item_ids)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT audit_events.* FROM audit_events \
         INNER JOIN ( \
           SELECT payload_item_id AS item_id,MAX(sequence) AS sequence \
           FROM audit_events \
           WHERE organization_id=? AND team_id=? AND session_id=? \
             AND event_type='plan.updated' \
             AND payload_item_id \
                 IN (SELECT value FROM json_each(?)) \
           GROUP BY payload_item_id \
         ) latest ON latest.sequence=audit_events.sequence",
    )
    .bind(&scope.organization_id.0)
    .bind(&scope.team_id.0)
    .bind(&session_id.0)
    .bind(item_ids)
    .fetch_all(pool)
    .await?)
}

async fn select_latest_agent_status_event_rows(
    pool: &SqlitePool,
    scope: &Scope,
    session_id: &Id,
    item_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let item_ids = serde_json::to_string(item_ids)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT audit_events.* FROM audit_events \
         INNER JOIN ( \
           SELECT payload_item_id AS item_id,MAX(sequence) AS sequence \
           FROM audit_events \
           WHERE organization_id=? AND team_id=? AND session_id=? \
             AND event_type='agent.status' \
             AND payload_item_id \
                 IN (SELECT value FROM json_each(?)) \
           GROUP BY payload_item_id \
         ) latest ON latest.sequence=audit_events.sequence",
    )
    .bind(&scope.organization_id.0)
    .bind(&scope.team_id.0)
    .bind(&session_id.0)
    .bind(item_ids)
    .fetch_all(pool)
    .await?)
}

async fn select_latest_hook_event_rows(
    pool: &SqlitePool,
    scope: &Scope,
    session_id: &Id,
    item_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if item_ids.is_empty() {
        return Ok(Vec::new());
    }
    let item_ids = serde_json::to_string(item_ids)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT audit_events.* FROM audit_events \
         INNER JOIN ( \
           SELECT payload_item_id AS item_id,MAX(sequence) AS sequence \
           FROM audit_events \
           WHERE organization_id=? AND team_id=? AND session_id=? \
             AND event_type IN ('hook.started','hook.completed','hook.failed') \
             AND payload_item_id \
                 IN (SELECT value FROM json_each(?)) \
           GROUP BY payload_item_id \
         ) latest ON latest.sequence=audit_events.sequence",
    )
    .bind(&scope.organization_id.0)
    .bind(&scope.team_id.0)
    .bind(&session_id.0)
    .bind(item_ids)
    .fetch_all(pool)
    .await?)
}

async fn select_latest_mcp_progress_event_rows(
    pool: &SqlitePool,
    scope: &Scope,
    session_id: &Id,
    tool_call_ids: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, StorageError> {
    if tool_call_ids.is_empty() {
        return Ok(Vec::new());
    }
    let tool_call_ids = serde_json::to_string(tool_call_ids)
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(sqlx::query(
        "SELECT audit_events.* FROM audit_events \
         INNER JOIN ( \
           SELECT payload_tool_call_id AS tool_call_id, \
                  MAX(sequence) AS sequence \
           FROM audit_events \
           WHERE organization_id=? AND team_id=? AND session_id=? \
             AND event_type='mcp.progress' \
             AND payload_tool_call_id \
                 IN (SELECT value FROM json_each(?)) \
           GROUP BY payload_tool_call_id \
         ) latest ON latest.sequence=audit_events.sequence",
    )
    .bind(&scope.organization_id.0)
    .bind(&scope.team_id.0)
    .bind(&session_id.0)
    .bind(tool_call_ids)
    .fetch_all(pool)
    .await?)
}

fn row_to_message(
    row: &sqlx::sqlite::SqliteRow,
    scope: &Scope,
    sensitive: &SensitiveCodec,
) -> Result<Message, StorageError> {
    let id = Id(row.try_get("id")?);
    let content: String = row.try_get("content_json")?;
    let content = sensitive.open_text(scope, "messages", &id, "content_json", &content)?;
    Ok(Message {
        id,
        session_id: Id(row.try_get("session_id")?),
        turn_id: Id(row.try_get("turn_id")?),
        role: row.try_get("role")?,
        content: serde_json::from_str(&content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_turn(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<Turn, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let checkpoint = row
        .try_get::<Option<String>, _>("checkpoint_json")?
        .map(|value| sensitive.open_text(&scope, "turns", &id, "checkpoint_json", &value))
        .transpose()?
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(Turn {
        id,
        session_id: Id(row.try_get("session_id")?),
        scope,
        status: parse_turn_status(&row.try_get::<String, _>("status")?)?,
        checkpoint,
        error_code: row.try_get("error_code")?,
        started_at: row.try_get("started_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn row_to_turn_input(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<TurnInput, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let content_json: String = row.try_get("content_json")?;
    let content_json =
        sensitive.open_text(&scope, "turn_inputs", &id, "content_json", &content_json)?;
    Ok(TurnInput {
        id,
        session_id: Id(row.try_get("session_id")?),
        target_turn_id: Id(row.try_get("target_turn_id")?),
        resulting_turn_id: row
            .try_get::<Option<String>, _>("resulting_turn_id")?
            .map(Id),
        scope,
        mode: parse_turn_input_mode(&row.try_get::<String, _>("mode")?)?,
        content: serde_json::from_str(&content_json)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?,
        status: parse_turn_input_status(&row.try_get::<String, _>("status")?)?,
        idempotency_key: row.try_get("idempotency_key")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        consumed_at: row.try_get("consumed_at")?,
        cancelled_at: row.try_get("cancelled_at")?,
        revision: row
            .try_get::<i64, _>("revision")?
            .try_into()
            .map_err(|_| StorageError::InvalidData("turn input revision is outside u64".into()))?,
    })
}

fn row_to_durable_task(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<DurableTask, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let json = |column: &str| -> Result<Option<serde_json::Value>, StorageError> {
        row.try_get::<Option<String>, _>(column)?
            .map(|value| sensitive.open_text(&scope, "durable_tasks", &id, column, &value))
            .transpose()?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))
    };
    Ok(DurableTask {
        id: id.clone(),
        scope: scope.clone(),
        kind: row.try_get("kind")?,
        payload: json("payload_json")?
            .ok_or_else(|| StorageError::InvalidData("durable task payload is missing".into()))?,
        idempotency_key: row.try_get("idempotency_key")?,
        status: parse_durable_task_status(&row.try_get::<String, _>("status")?)?,
        attempt: u32::try_from(row.try_get::<i64, _>("attempt")?)
            .map_err(|_| StorageError::InvalidData("invalid attempt".into()))?,
        max_attempts: u32::try_from(row.try_get::<i64, _>("max_attempts")?)
            .map_err(|_| StorageError::InvalidData("invalid max attempts".into()))?,
        max_runtime_seconds: u64::try_from(row.try_get::<i64, _>("max_runtime_seconds")?)
            .map_err(|_| StorageError::InvalidData("invalid runtime budget".into()))?,
        max_cost_micros: u64::try_from(row.try_get::<i64, _>("max_cost_micros")?)
            .map_err(|_| StorageError::InvalidData("invalid cost budget".into()))?,
        consumed_cost_micros: u64::try_from(row.try_get::<i64, _>("consumed_cost_micros")?)
            .map_err(|_| StorageError::InvalidData("invalid consumed cost".into()))?,
        max_runner_cost_micros: u64::try_from(row.try_get::<i64, _>("max_runner_cost_micros")?)
            .map_err(|_| StorageError::InvalidData("invalid runner cost budget".into()))?,
        consumed_runner_cost_micros: u64::try_from(
            row.try_get::<i64, _>("consumed_runner_cost_micros")?,
        )
        .map_err(|_| StorageError::InvalidData("invalid consumed runner cost".into()))?,
        lease_owner: row.try_get("lease_owner")?,
        lease_token: row.try_get("lease_token")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        checkpoint: json("checkpoint_json")?,
        result: json("result_json")?,
        error: row
            .try_get::<Option<String>, _>("error")?
            .map(|value| sensitive.open_text(&scope, "durable_tasks", &id, "error", &value))
            .transpose()?,
        cancel_requested: row.try_get::<i64, _>("cancel_requested")? != 0,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn durable_task_status_str(status: &DurableTaskStatus) -> &'static str {
    match status {
        DurableTaskStatus::Queued => "queued",
        DurableTaskStatus::Leased => "leased",
        DurableTaskStatus::Running => "running",
        DurableTaskStatus::Paused => "paused",
        DurableTaskStatus::Succeeded => "succeeded",
        DurableTaskStatus::Failed => "failed",
        DurableTaskStatus::Cancelled => "cancelled",
    }
}

fn parse_durable_task_status(value: &str) -> Result<DurableTaskStatus, StorageError> {
    match value {
        "queued" => Ok(DurableTaskStatus::Queued),
        "leased" => Ok(DurableTaskStatus::Leased),
        "running" => Ok(DurableTaskStatus::Running),
        "paused" => Ok(DurableTaskStatus::Paused),
        "succeeded" => Ok(DurableTaskStatus::Succeeded),
        "failed" => Ok(DurableTaskStatus::Failed),
        "cancelled" => Ok(DurableTaskStatus::Cancelled),
        _ => Err(StorageError::InvalidData(format!(
            "invalid durable task status {value}"
        ))),
    }
}

fn background_terminal_status_str(status: &BackgroundTerminalStatus) -> &'static str {
    match status {
        BackgroundTerminalStatus::Starting => "starting",
        BackgroundTerminalStatus::Running => "running",
        BackgroundTerminalStatus::Exited => "exited",
        BackgroundTerminalStatus::Failed => "failed",
        BackgroundTerminalStatus::Stopped => "stopped",
        BackgroundTerminalStatus::Orphaned => "orphaned",
    }
}

fn parse_background_terminal_status(value: &str) -> Result<BackgroundTerminalStatus, StorageError> {
    match value {
        "starting" => Ok(BackgroundTerminalStatus::Starting),
        "running" => Ok(BackgroundTerminalStatus::Running),
        "exited" => Ok(BackgroundTerminalStatus::Exited),
        "failed" => Ok(BackgroundTerminalStatus::Failed),
        "stopped" => Ok(BackgroundTerminalStatus::Stopped),
        "orphaned" => Ok(BackgroundTerminalStatus::Orphaned),
        _ => Err(StorageError::InvalidData(format!(
            "invalid background terminal status {value}"
        ))),
    }
}

async fn settle_durable_budget(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &Id,
    model_delta_micros: u64,
    runner_delta_micros: u64,
    terminal: bool,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    let Some(row) = sqlx::query("SELECT budget_id,reserved_remaining_micros,reserved_runner_remaining_micros FROM team_budget_reservations WHERE durable_task_id=? AND status='active'")
        .bind(&task_id.0).fetch_optional(&mut **tx).await? else {
        return Ok(());
    };
    let budget_id: String = row.try_get("budget_id")?;
    let remaining = u64::try_from(row.try_get::<i64, _>("reserved_remaining_micros")?)
        .map_err(|_| StorageError::InvalidData("negative budget reservation".into()))?;
    let runner_remaining =
        u64::try_from(row.try_get::<i64, _>("reserved_runner_remaining_micros")?)
            .map_err(|_| StorageError::InvalidData("negative runner budget reservation".into()))?;
    if model_delta_micros > remaining || runner_delta_micros > runner_remaining {
        return Err(StorageError::InvalidState(
            "task consumption exceeds its Team budget reservation".into(),
        ));
    }
    let model_released = if terminal {
        remaining
    } else {
        model_delta_micros
    };
    let runner_released = if terminal {
        runner_remaining
    } else {
        runner_delta_micros
    };
    let model_delta = i64::try_from(model_delta_micros)
        .map_err(|_| StorageError::InvalidData("cost is too large".into()))?;
    let runner_delta = i64::try_from(runner_delta_micros)
        .map_err(|_| StorageError::InvalidData("runner cost is too large".into()))?;
    let model_released = i64::try_from(model_released)
        .map_err(|_| StorageError::InvalidData("reservation is too large".into()))?;
    let runner_released = i64::try_from(runner_released)
        .map_err(|_| StorageError::InvalidData("runner reservation is too large".into()))?;
    let changed = sqlx::query("UPDATE team_budgets SET model_consumed_micros=model_consumed_micros+?,runner_consumed_micros=runner_consumed_micros+?,model_reserved_micros=model_reserved_micros-?,runner_reserved_micros=runner_reserved_micros-?,updated_at=? WHERE id=? AND model_reserved_micros>=? AND runner_reserved_micros>=?")
        .bind(model_delta).bind(runner_delta).bind(model_released).bind(runner_released).bind(now).bind(&budget_id).bind(model_released).bind(runner_released).execute(&mut **tx).await?;
    if changed.rows_affected() != 1 {
        return Err(StorageError::InvalidState(
            "Team budget reservation changed concurrently".into(),
        ));
    }
    sqlx::query("UPDATE team_budget_reservations SET reserved_remaining_micros=CASE WHEN ? THEN 0 ELSE reserved_remaining_micros-? END,reserved_runner_remaining_micros=CASE WHEN ? THEN 0 ELSE reserved_runner_remaining_micros-? END,settled_micros=settled_micros+?,settled_runner_micros=settled_runner_micros+?,status=CASE WHEN ? THEN 'settled' ELSE status END,updated_at=? WHERE durable_task_id=? AND status='active'")
        .bind(terminal).bind(model_delta).bind(terminal).bind(runner_delta).bind(model_delta).bind(runner_delta).bind(terminal).bind(now).bind(&task_id.0).execute(&mut **tx).await?;
    Ok(())
}

fn row_to_team_goal(row: &sqlx::sqlite::SqliteRow) -> Result<TeamGoal, StorageError> {
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "planned" => GoalStatus::Planned,
        "active" => GoalStatus::Active,
        "achieved" => GoalStatus::Achieved,
        "cancelled" => GoalStatus::Cancelled,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid goal status {value}"
            )));
        }
    };
    Ok(TeamGoal {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        title: row.try_get("title")?,
        outcome_definition: row.try_get("outcome_definition")?,
        status,
        target_date: row.try_get("target_date")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_team_goal_run(row: &sqlx::sqlite::SqliteRow) -> Result<TeamGoalRun, StorageError> {
    let max_attempts = u32::try_from(row.try_get::<i64, _>("max_attempts")?)
        .map_err(|_| StorageError::InvalidData("invalid Goal run attempts".into()))?;
    let max_runtime_seconds = u64::try_from(row.try_get::<i64, _>("max_runtime_seconds")?)
        .map_err(|_| StorageError::InvalidData("invalid Goal run runtime".into()))?;
    let max_cost_micros = u64::try_from(row.try_get::<i64, _>("max_cost_micros")?)
        .map_err(|_| StorageError::InvalidData("invalid Goal run cost".into()))?;
    let revision = u64::try_from(row.try_get::<i64, _>("revision")?)
        .map_err(|_| StorageError::InvalidData("invalid Goal run revision".into()))?;
    Ok(TeamGoalRun {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        goal_id: Id(row.try_get("goal_id")?),
        status: parse_team_goal_run_status(&row.try_get::<String, _>("status")?)?,
        workspace_uri: row.try_get("workspace_uri")?,
        model: row.try_get("model")?,
        max_attempts,
        max_runtime_seconds,
        max_cost_micros,
        current_task_id: row.try_get::<Option<String>, _>("current_task_id")?.map(Id),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        revision,
    })
}

fn row_to_team_task(row: &sqlx::sqlite::SqliteRow) -> Result<TeamTask, StorageError> {
    Ok(TeamTask {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
        source: row.try_get("source")?,
        title: row.try_get("title")?,
        priority: row.try_get("priority")?,
        assignee_type: row.try_get("assignee_type")?,
        assignee_id: row.try_get::<Option<String>, _>("assignee_id")?.map(Id),
        status: parse_task_status(&row.try_get::<String, _>("status")?)?,
        acceptance_criteria: parse_json_column(row, "acceptance_criteria_json")?,
        required_evidence: parse_json_column(row, "required_evidence_json")?,
        blockers: parse_json_column(row, "blockers_json")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_team_knowledge(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<TeamKnowledgeItem, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_team_scope(row)?;
    let content: String = row.try_get("content")?;
    Ok(TeamKnowledgeItem {
        id: id.clone(),
        scope: scope.clone(),
        source_uri: row.try_get("source_uri")?,
        title: row.try_get("title")?,
        content: sensitive.open_text(&scope, "team_knowledge", &id, "content", &content)?,
        version: row.try_get("version")?,
        trust_level: row.try_get("trust_level")?,
        permission: row.try_get("permission")?,
        valid_until: row.try_get("valid_until")?,
        owner_team_id: Id(row.try_get("owner_team_id")?),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_team_outcome(row: &sqlx::sqlite::SqliteRow) -> Result<TeamOutcome, StorageError> {
    Ok(TeamOutcome {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        goal_id: Id(row.try_get("goal_id")?),
        task_id: Id(row.try_get("task_id")?),
        status: row.try_get("status")?,
        evidence: parse_json_column::<Vec<OutcomeEvidence>>(row, "evidence_json")?,
        pull_request_url: row.try_get("pull_request_url")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn row_to_team_ownership(row: &sqlx::sqlite::SqliteRow) -> Result<TeamOwnership, StorageError> {
    Ok(TeamOwnership {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        resource_type: row.try_get("resource_type")?,
        resource_uri: row.try_get("resource_uri")?,
        service_tier: row.try_get("service_tier")?,
        on_call: row.try_get("on_call")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        archived_at: row.try_get("archived_at")?,
    })
}

fn row_to_team_capacity(row: &sqlx::sqlite::SqliteRow) -> Result<TeamCapacity, StorageError> {
    let agent_concurrency = u32::try_from(row.try_get::<i64, _>("agent_concurrency")?)
        .map_err(|_| StorageError::InvalidData("invalid stored agent concurrency".into()))?;
    let wip_limit = u32::try_from(row.try_get::<i64, _>("wip_limit")?)
        .map_err(|_| StorageError::InvalidData("invalid stored WIP limit".into()))?;
    Ok(TeamCapacity {
        scope: row_team_scope(row)?,
        human_available_hours: row.try_get("human_available_hours")?,
        agent_concurrency,
        wip_limit,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_team_budget(row: &sqlx::sqlite::SqliteRow) -> Result<TeamBudget, StorageError> {
    let unsigned = |column: &str| -> Result<u64, StorageError> {
        u64::try_from(row.try_get::<i64, _>(column)?).map_err(|_| {
            StorageError::InvalidData(format!("negative stored budget value: {column}"))
        })
    };
    Ok(TeamBudget {
        id: Id(row.try_get("id")?),
        scope: row_team_scope(row)?,
        period_start: row.try_get("period_start")?,
        period_end: row.try_get("period_end")?,
        model_limit_micros: unsigned("model_limit_micros")?,
        runner_limit_micros: unsigned("runner_limit_micros")?,
        model_consumed_micros: unsigned("model_consumed_micros")?,
        runner_consumed_micros: unsigned("runner_consumed_micros")?,
        model_reserved_micros: unsigned("model_reserved_micros")?,
        runner_reserved_micros: unsigned("runner_reserved_micros")?,
        hard_limit: row.try_get("hard_limit")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_team_scope(row: &sqlx::sqlite::SqliteRow) -> Result<Scope, StorageError> {
    Ok(Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    })
}

fn json_text<T: serde::Serialize>(value: &T) -> Result<String, StorageError> {
    serde_json::to_string(value).map_err(|error| StorageError::InvalidData(error.to_string()))
}

fn database_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_settings(settings: &DaemonSettings) -> Result<(), StorageError> {
    let values = [
        settings.organization_id.0.as_str(),
        settings.team_id.0.as_str(),
        settings.actor_id.0.as_str(),
        settings.workspace_uri.as_str(),
        settings.default_model.as_str(),
        settings.default_title.as_str(),
    ];
    let sensitive = |value: &str| {
        let lower = value.trim().to_ascii_lowercase();
        let raw_api_key = lower.strip_prefix("sk-").is_some_and(|suffix| {
            suffix.len() >= 16
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        });
        raw_api_key
            || lower.contains("api_key=")
            || lower.contains("api-key=")
            || lower.contains("token=")
            || lower.contains("authorization:")
            || lower.contains("authorization=")
            || lower.contains("bearer ")
            || lower.contains("password=")
            || lower.contains("secret=")
    };
    if values.iter().any(|value| value.trim().is_empty())
        || values.iter().any(|value| value.len() > 4096)
        || values.iter().any(|value| sensitive(value))
        || settings.max_context_tokens < 1_024
        || settings.max_context_tokens > 2_000_000
        || url::Url::parse(&settings.workspace_uri).is_err()
    {
        return Err(StorageError::InvalidData(
            "settings are invalid, unsafe or contain secret-like data".into(),
        ));
    }
    Ok(())
}

fn parse_json_column<T: serde::de::DeserializeOwned>(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<T, StorageError> {
    let encoded: String = row.try_get(column)?;
    serde_json::from_str(&encoded).map_err(|error| StorageError::InvalidData(error.to_string()))
}

fn task_status_str(value: &TeamTaskStatus) -> &'static str {
    match value {
        TeamTaskStatus::Ready => "ready",
        TeamTaskStatus::InProgress => "in_progress",
        TeamTaskStatus::Blocked => "blocked",
        TeamTaskStatus::Review => "review",
        TeamTaskStatus::Verified => "verified",
        TeamTaskStatus::Cancelled => "cancelled",
    }
}

fn goal_status_str(value: &GoalStatus) -> &'static str {
    match value {
        GoalStatus::Planned => "planned",
        GoalStatus::Active => "active",
        GoalStatus::Achieved => "achieved",
        GoalStatus::Cancelled => "cancelled",
    }
}

fn team_goal_run_status_str(value: &TeamGoalRunStatus) -> &'static str {
    match value {
        TeamGoalRunStatus::Active => "active",
        TeamGoalRunStatus::Paused => "paused",
        TeamGoalRunStatus::Cancelled => "cancelled",
        TeamGoalRunStatus::AwaitingVerification => "awaiting_verification",
    }
}

fn parse_team_goal_run_status(value: &str) -> Result<TeamGoalRunStatus, StorageError> {
    match value {
        "active" => Ok(TeamGoalRunStatus::Active),
        "paused" => Ok(TeamGoalRunStatus::Paused),
        "cancelled" => Ok(TeamGoalRunStatus::Cancelled),
        "awaiting_verification" => Ok(TeamGoalRunStatus::AwaitingVerification),
        value => Err(StorageError::InvalidData(format!(
            "invalid Team Goal run status {value}"
        ))),
    }
}

fn validate_team_goal_run_transition(
    current: &TeamGoalRunStatus,
    next: &TeamGoalRunStatus,
) -> Result<(), StorageError> {
    use TeamGoalRunStatus::*;
    let valid = current == next
        || matches!(
            (current, next),
            (Active, Paused | Cancelled | AwaitingVerification)
                | (Paused, Active | Cancelled)
                | (AwaitingVerification, Active | Cancelled)
        );
    if !valid {
        return Err(StorageError::InvalidState(
            "invalid or terminal Team Goal run transition".into(),
        ));
    }
    Ok(())
}

fn parse_task_status(value: &str) -> Result<TeamTaskStatus, StorageError> {
    match value {
        "ready" => Ok(TeamTaskStatus::Ready),
        "in_progress" => Ok(TeamTaskStatus::InProgress),
        "blocked" => Ok(TeamTaskStatus::Blocked),
        "review" => Ok(TeamTaskStatus::Review),
        "verified" => Ok(TeamTaskStatus::Verified),
        "cancelled" => Ok(TeamTaskStatus::Cancelled),
        value => Err(StorageError::InvalidData(format!(
            "invalid task status {value}"
        ))),
    }
}

fn valid_task_transition(from: &TeamTaskStatus, to: &TeamTaskStatus) -> bool {
    use TeamTaskStatus::*;
    matches!(
        (from, to),
        (Ready, InProgress | Blocked | Cancelled)
            | (InProgress, Blocked | Review | Cancelled)
            | (Blocked, Ready | InProgress | Cancelled)
            | (Review, InProgress | Blocked | Cancelled)
    ) || from == to
}

fn turn_status_str(value: &TurnStatus) -> &'static str {
    match value {
        TurnStatus::Idle => "idle",
        TurnStatus::PreparingContext => "preparing_context",
        TurnStatus::CallingModel => "calling_model",
        TurnStatus::AwaitingInput => "awaiting_input",
        TurnStatus::AwaitingApproval => "awaiting_approval",
        TurnStatus::RunningTool => "running_tool",
        TurnStatus::Completed => "completed",
        TurnStatus::Failed => "failed",
        TurnStatus::Cancelled => "cancelled",
    }
}

fn parse_turn_status(value: &str) -> Result<TurnStatus, StorageError> {
    match value {
        "idle" => Ok(TurnStatus::Idle),
        "preparing_context" => Ok(TurnStatus::PreparingContext),
        "calling_model" => Ok(TurnStatus::CallingModel),
        "awaiting_input" => Ok(TurnStatus::AwaitingInput),
        "awaiting_approval" => Ok(TurnStatus::AwaitingApproval),
        "running_tool" => Ok(TurnStatus::RunningTool),
        "completed" => Ok(TurnStatus::Completed),
        "failed" => Ok(TurnStatus::Failed),
        "cancelled" => Ok(TurnStatus::Cancelled),
        value => Err(StorageError::InvalidData(format!(
            "invalid turn status {value}"
        ))),
    }
}

fn turn_input_mode_str(value: &TurnInputMode) -> &'static str {
    match value {
        TurnInputMode::Steer => "steer",
        TurnInputMode::Queue => "queue",
    }
}

fn parse_turn_input_mode(value: &str) -> Result<TurnInputMode, StorageError> {
    match value {
        "steer" => Ok(TurnInputMode::Steer),
        "queue" => Ok(TurnInputMode::Queue),
        value => Err(StorageError::InvalidData(format!(
            "invalid turn input mode {value}"
        ))),
    }
}

fn parse_turn_input_status(value: &str) -> Result<TurnInputStatus, StorageError> {
    match value {
        "pending" => Ok(TurnInputStatus::Pending),
        "processing" => Ok(TurnInputStatus::Processing),
        "consumed" => Ok(TurnInputStatus::Consumed),
        "cancelled" => Ok(TurnInputStatus::Cancelled),
        value => Err(StorageError::InvalidData(format!(
            "invalid turn input status {value}"
        ))),
    }
}

fn is_terminal(value: &TurnStatus) -> bool {
    matches!(
        value,
        TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
    )
}

fn ensure_team_scope(expected: &Scope, actual: &Scope) -> Result<(), StorageError> {
    if expected.organization_id != actual.organization_id || expected.team_id != actual.team_id {
        return Err(StorageError::ScopeMismatch);
    }
    Ok(())
}

fn ensure_actor_session_scope(session: &Session, actual: &Scope) -> Result<(), StorageError> {
    ensure_actor_scope(&session.scope, actual)
}

fn ensure_actor_scope(expected: &Scope, actual: &Scope) -> Result<(), StorageError> {
    ensure_team_scope(expected, actual)?;
    if expected.actor_id != actual.actor_id {
        return Err(StorageError::ScopeMismatch);
    }
    Ok(())
}

fn validate_session_goal_text<'a>(
    field: &str,
    value: &'a str,
    max_chars: usize,
) -> Result<&'a str, StorageError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > max_chars
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(StorageError::InvalidData(format!(
            "session goal {field} must contain 1 to {max_chars} safe characters"
        )));
    }
    Ok(value)
}

fn validate_session_goal_transition(
    current: &SessionGoalStatus,
    next: &SessionGoalStatus,
) -> Result<(), StorageError> {
    use SessionGoalStatus::*;
    let valid = current == next
        || matches!(
            (current, next),
            (Active, Paused | Completed | Blocked)
                | (Paused, Active | Completed | Blocked)
                | (Blocked, Active | Completed)
        );
    if !valid {
        return Err(StorageError::InvalidState(
            "invalid or terminal session goal transition".into(),
        ));
    }
    Ok(())
}

fn session_goal_status_str(status: &SessionGoalStatus) -> &'static str {
    match status {
        SessionGoalStatus::Active => "active",
        SessionGoalStatus::Paused => "paused",
        SessionGoalStatus::Completed => "completed",
        SessionGoalStatus::Blocked => "blocked",
    }
}

fn session_goal_conversation_sha256(goal: Option<&SessionGoal>) -> Result<String, StorageError> {
    let value = goal.map(|goal| {
        serde_json::json!({
            "id": goal.id,
            "objective": goal.objective,
            "status": goal.status,
            "auto_continue": goal.auto_continue,
            "token_budget": goal.token_budget,
            "blocked_reason": goal.blocked_reason,
        })
    });
    let encoded =
        serde_json::to_vec(&value).map_err(|error| StorageError::InvalidData(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn valid_knowledge_permission(permission: &str) -> bool {
    permission == "team"
        || permission
            .strip_prefix("actor:")
            .is_some_and(|actor| !actor.is_empty() && !actor.chars().any(char::is_whitespace))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_attachment_name(value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.len() > 255
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains('\\')
        || matches!(value, "." | "..")
    {
        return Err(StorageError::InvalidData(
            "attachment file name is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_attachment_media_type(value: &str) -> Result<(), StorageError> {
    if value.is_empty()
        || value.len() > 127
        || !value.is_ascii()
        || value.chars().any(char::is_whitespace)
        || !value.contains('/')
    {
        return Err(StorageError::InvalidData(
            "attachment media type is invalid".into(),
        ));
    }
    Ok(())
}

fn decision_str(value: &PolicyDecision) -> &'static str {
    match value {
        PolicyDecision::Allow => "allow",
        PolicyDecision::Ask => "ask",
        PolicyDecision::Deny => "deny",
    }
}

fn parse_decision(value: &str) -> Result<PolicyDecision, StorageError> {
    match value {
        "allow" => Ok(PolicyDecision::Allow),
        "ask" => Ok(PolicyDecision::Ask),
        "deny" => Ok(PolicyDecision::Deny),
        value => Err(StorageError::InvalidData(format!(
            "invalid policy decision {value}"
        ))),
    }
}

fn tool_status_str(value: &ToolCallStatus) -> &'static str {
    match value {
        ToolCallStatus::Proposed => "proposed",
        ToolCallStatus::AwaitingApproval => "awaiting_approval",
        ToolCallStatus::Running => "running",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Denied => "denied",
        ToolCallStatus::Failed => "failed",
        ToolCallStatus::Cancelled => "cancelled",
    }
}

fn parse_tool_status(value: &str) -> Result<ToolCallStatus, StorageError> {
    match value {
        "proposed" => Ok(ToolCallStatus::Proposed),
        "awaiting_approval" => Ok(ToolCallStatus::AwaitingApproval),
        "running" => Ok(ToolCallStatus::Running),
        "completed" => Ok(ToolCallStatus::Completed),
        "denied" => Ok(ToolCallStatus::Denied),
        "failed" => Ok(ToolCallStatus::Failed),
        "cancelled" => Ok(ToolCallStatus::Cancelled),
        value => Err(StorageError::InvalidData(format!(
            "invalid tool status {value}"
        ))),
    }
}

fn approval_scope_str(value: &ApprovalScope) -> &'static str {
    match value {
        ApprovalScope::Once => "once",
        ApprovalScope::Session => "session",
    }
}

fn row_scope(row: &sqlx::sqlite::SqliteRow) -> Result<Scope, StorageError> {
    Ok(Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: row.try_get::<Option<String>, _>("goal_id")?.map(Id),
        task_id: row.try_get::<Option<String>, _>("task_id")?.map(Id),
    })
}

fn row_to_tool_call(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<ToolCall, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let arguments: String = row.try_get("arguments_json")?;
    let arguments = sensitive.open_text(&scope, "tool_calls", &id, "arguments_json", &arguments)?;
    let result = row
        .try_get::<Option<String>, _>("result_json")?
        .map(|value| sensitive.open_text(&scope, "tool_calls", &id, "result_json", &value))
        .transpose()?
        .map(|value| serde_json::from_str(&value))
        .transpose()
        .map_err(|error| StorageError::InvalidData(error.to_string()))?;
    let decision = parse_decision(&row.try_get::<String, _>("policy_decision")?)?;
    Ok(ToolCall {
        request: ToolRequest {
            id: id.clone(),
            scope: scope.clone(),
            session_id: Id(row.try_get("session_id")?),
            turn_id: Id(row.try_get("turn_id")?),
            tool: row.try_get("tool")?,
            arguments: serde_json::from_str(&arguments)
                .map_err(|error| StorageError::InvalidData(error.to_string()))?,
            created_at: row.try_get("created_at")?,
        },
        policy: PolicyResult {
            requires_approval: decision == PolicyDecision::Ask,
            decision,
            policy_id: row.try_get("policy_id")?,
            policy_version: row.try_get("policy_version")?,
            reason: row.try_get("policy_reason")?,
        },
        simulated_policy: row
            .try_get::<Option<String>, _>("simulated_policy_json")?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?,
        central_policy_applied: row.try_get("central_policy_applied")?,
        team_configuration_sequence: row
            .try_get::<Option<i64>, _>("team_configuration_sequence")?
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    StorageError::InvalidData("negative configuration sequence".into())
                })
            })
            .transpose()?,
        status: parse_tool_status(&row.try_get::<String, _>("status")?)?,
        result,
        error: row
            .try_get::<Option<String>, _>("error")?
            .map(|value| sensitive.open_text(&scope, "tool_calls", &id, "error", &value))
            .transpose()?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn row_to_approval(row: &sqlx::sqlite::SqliteRow) -> Result<Approval, StorageError> {
    let approval_scope = match row.try_get::<String, _>("approval_scope")?.as_str() {
        "once" => ApprovalScope::Once,
        "session" => ApprovalScope::Session,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid approval scope {value}"
            )));
        }
    };
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "pending" => ApprovalStatus::Pending,
        "approved" => ApprovalStatus::Approved,
        "rejected" => ApprovalStatus::Rejected,
        "expired" => ApprovalStatus::Expired,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid approval status {value}"
            )));
        }
    };
    Ok(Approval {
        id: Id(row.try_get("id")?),
        tool_call_id: Id(row.try_get("tool_call_id")?),
        scope: row_scope(row)?,
        approval_scope,
        status,
        requested_at: row.try_get("requested_at")?,
        decided_at: row.try_get("decided_at")?,
        decided_by: row.try_get::<Option<String>, _>("decided_by")?.map(Id),
    })
}

fn row_to_question_request(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<QuestionRequest, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let questions = row.try_get::<String, _>("questions_json")?;
    let questions = sensitive.open_text(
        &scope,
        "question_requests",
        &id,
        "questions_json",
        &questions,
    )?;
    let answers = row
        .try_get::<Option<String>, _>("answers_json")?
        .map(|answers| {
            sensitive.open_text(&scope, "question_requests", &id, "answers_json", &answers)
        })
        .transpose()?;
    let status = match row.try_get::<String, _>("status")?.as_str() {
        "pending" => QuestionStatus::Pending,
        "answered" => QuestionStatus::Answered,
        "expired" => QuestionStatus::Expired,
        "cancelled" => QuestionStatus::Cancelled,
        value => {
            return Err(StorageError::InvalidData(format!(
                "invalid question status {value}"
            )));
        }
    };
    Ok(QuestionRequest {
        id,
        session_id: Id(row.try_get("session_id")?),
        turn_id: Id(row.try_get("turn_id")?),
        item_id: Id(row.try_get("item_id")?),
        questions: serde_json::from_str(&questions)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?,
        allow_other: row.try_get("allow_other")?,
        requested_by: scope.actor_id,
        requested_at: row.try_get("requested_at")?,
        expires_at: row.try_get("expires_at")?,
        status,
        answers: answers
            .map(|answers| serde_json::from_str(&answers))
            .transpose()
            .map_err(|error| StorageError::InvalidData(error.to_string()))?
            .unwrap_or_default(),
        answered_by: row.try_get::<Option<String>, _>("answered_by")?.map(Id),
        answered_at: row.try_get("answered_at")?,
        revision: u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("negative question revision".into()))?,
    })
}

fn row_to_artifact(
    row: &sqlx::sqlite::SqliteRow,
    sensitive: &SensitiveCodec,
) -> Result<Artifact, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = row_scope(row)?;
    let title = sensitive.open_text(
        &scope,
        "artifacts",
        &id,
        "title",
        &row.try_get::<String, _>("title")?,
    )?;
    let content = sensitive.open_text(
        &scope,
        "artifacts",
        &id,
        "content_json",
        &row.try_get::<String, _>("content_json")?,
    )?;
    Ok(Artifact {
        metadata: opencoding_protocol::ArtifactMetadata {
            id,
            session_id: Id(row.try_get("session_id")?),
            turn_id: Id(row.try_get("turn_id")?),
            item_id: Id(row.try_get("item_id")?),
            title,
            media_type: row.try_get("media_type")?,
            byte_length: Some(
                u64::try_from(row.try_get::<i64, _>("byte_length")?)
                    .map_err(|_| StorageError::InvalidData("negative artifact length".into()))?,
            ),
            created_at: row.try_get("created_at")?,
        },
        content: serde_json::from_str(&content)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?,
    })
}

fn row_to_attachment(
    row: &sqlx::sqlite::SqliteRow,
    expected_scope: &Scope,
    sensitive: &SensitiveCodec,
) -> Result<Attachment, StorageError> {
    let id = Id(row.try_get("id")?);
    let scope = Scope {
        organization_id: Id(row.try_get("organization_id")?),
        team_id: Id(row.try_get("team_id")?),
        actor_id: Id(row.try_get("actor_id")?),
        goal_id: None,
        task_id: None,
    };
    ensure_team_scope(&scope, expected_scope)?;
    let turn_id = row.try_get::<Option<String>, _>("turn_id")?.map(Id);
    if turn_id.is_none() && scope.actor_id != expected_scope.actor_id {
        return Err(StorageError::ScopeMismatch);
    }
    let file_name = sensitive.open_text(
        &scope,
        "attachments",
        &id,
        "file_name",
        &row.try_get::<String, _>("file_name")?,
    )?;
    let media_type = sensitive.open_text(
        &scope,
        "attachments",
        &id,
        "media_type",
        &row.try_get::<String, _>("media_type")?,
    )?;
    let content_base64 = sensitive.open_text(
        &scope,
        "attachments",
        &id,
        "content_base64",
        &row.try_get::<String, _>("content_base64")?,
    )?;
    let sha256 = sensitive.open_text(
        &scope,
        "attachments",
        &id,
        "sha256",
        &row.try_get::<String, _>("sha256")?,
    )?;
    Ok(Attachment {
        metadata: AttachmentMetadata {
            id,
            session_id: Id(row.try_get("session_id")?),
            turn_id,
            file_name,
            media_type,
            byte_length: u64::try_from(row.try_get::<i64, _>("byte_length")?)
                .map_err(|_| StorageError::InvalidData("negative attachment length".into()))?,
            sha256,
            created_at: row.try_get("created_at")?,
        },
        content_base64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use opencoding_audit::{
        CENTRAL_AUDIT_SCHEMA_VERSION, CentralAuditBatchPayload, CentralAuditRecord,
        CentralAuditSigner,
    };
    use std::sync::Arc;

    fn private_tempdir() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(
            directory.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        directory
    }

    fn scope(team: &str) -> Scope {
        Scope {
            organization_id: Id("org_1".into()),
            team_id: Id(team.into()),
            actor_id: Id("usr_1".into()),
            goal_id: None,
            task_id: None,
        }
    }

    #[tokio::test]
    async fn local_device_identity_is_stable() {
        let store = Store::in_memory().await.unwrap();
        let first = store.get_or_create_device_id().await.unwrap();
        let second = store.get_or_create_device_id().await.unwrap();
        assert_eq!(first, second);
        assert!(first.0.starts_with("device_"));
    }

    #[tokio::test]
    async fn reasoning_summary_deltas_are_encrypted_bounded_and_finalize_in_one_item() {
        let store = Store::connect_encrypted("sqlite::memory:", "reasoning-key", &[8_u8; 32])
            .await
            .unwrap();
        let team = scope("team_reasoning");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "Reasoning".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let item_id = Id("item-reasoning".into());
        store
            .append_reasoning_summary_delta(&team, &session.id, &turn.id, &item_id, "Checked ")
            .await
            .unwrap()
            .unwrap();
        let message = store
            .append_reasoning_summary_delta(
                &team,
                &session.id,
                &turn.id,
                &item_id,
                "the constraints.",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            message.content,
            serde_json::json!("Checked the constraints.")
        );
        let raw = sqlx::query_scalar::<_, String>("SELECT content_json FROM messages WHERE id=?")
            .bind(&item_id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:v1:reasoning-key:"));
        assert!(!raw.contains("constraints"));
        assert!(
            store
                .complete_reasoning_summary(&team, &session.id, &turn.id, &item_id)
                .await
                .unwrap()
        );
        let stored = store.list_messages(&team, &session.id).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].role, "reasoning_summary");
        let page = store
            .list_transcript_item_references(&team, &session.id, None, 10)
            .await
            .unwrap();
        assert_eq!(page.references.len(), 1);
        assert_eq!(page.references[0].item_id, item_id);
    }

    #[tokio::test]
    async fn transcript_index_pages_heterogeneous_sources_without_replaying_plan_revisions() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "paged transcript".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let message = store
            .append_turn_message(
                &team,
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("first"),
            )
            .await
            .unwrap();
        let artifact = Artifact {
            metadata: opencoding_protocol::ArtifactMetadata {
                id: Id("artifact-page".into()),
                session_id: session.id.clone(),
                turn_id: turn.id.clone(),
                item_id: Id("item-artifact-page".into()),
                title: "Paged artifact".into(),
                media_type: "text/markdown".into(),
                byte_length: Some(4),
                created_at: Utc::now(),
            },
            content: serde_json::json!("body"),
        };
        store.create_artifact(&team, &artifact).await.unwrap();
        for (sequence, title) in [(1, "draft"), (2, "final")] {
            store
                .append_event(
                    &Event {
                        id: Id(format!("event-plan-{sequence}")),
                        sequence,
                        timestamp: Utc::now(),
                        scope: team.clone(),
                        session_id: Some(session.id.clone()),
                        turn_id: Some(turn.id.clone()),
                        kind: "plan.updated".into(),
                        payload: serde_json::json!({
                            "item_id": "item-plan-page",
                            "title": title,
                            "steps": [],
                        }),
                    },
                    &format!("{sequence:064x}"),
                )
                .await
                .unwrap();
        }

        let newest = store
            .list_transcript_item_references(&team, &session.id, None, 2)
            .await
            .unwrap();
        assert_eq!(newest.total_count, 3);
        assert!(newest.has_older);
        assert_eq!(newest.references.len(), 2);
        let older = store
            .list_transcript_item_references(
                &team,
                &session.id,
                newest
                    .references
                    .first()
                    .map(|reference| (&reference.created_at, &reference.item_id)),
                2,
            )
            .await
            .unwrap();
        assert_eq!(older.references.len(), 1);
        let ids = newest
            .references
            .iter()
            .chain(&older.references)
            .map(|reference| reference.item_id.clone())
            .collect::<HashSet<_>>();
        assert_eq!(
            ids,
            HashSet::from([
                message.id,
                artifact.metadata.item_id.clone(),
                Id("item-plan-page".into()),
            ])
        );

        let all = store
            .list_transcript_item_references(&team, &session.id, None, 3)
            .await
            .unwrap();
        let sources = store
            .load_transcript_page_sources(&team, &session.id, &all.references)
            .await
            .unwrap();
        assert_eq!(sources.messages.len(), 1);
        assert_eq!(sources.artifacts.len(), 1);
        assert_eq!(sources.events.len(), 1);
        assert_eq!(sources.events[0].payload["title"], "final");
        assert_eq!(sources.events[0].sequence, 2);
    }

    #[tokio::test]
    async fn settings_survive_database_restart_and_reject_secret_like_values() {
        let dir = private_tempdir();
        let path = dir.path().join("settings.sqlite");
        let url = format!("sqlite://{}", path.display());
        let store = Store::connect(&url).await.unwrap();
        let settings = DaemonSettings {
            workspace_uri: "file:///repo".into(),
            default_model: "approved-model".into(),
            max_context_tokens: 64_000,
            ..DaemonSettings::default()
        };
        store.put_settings(&settings).await.unwrap();
        let mut common_project_name = settings.clone();
        common_project_name.workspace_uri = "file:///repo/durable-task-queue".into();
        common_project_name.default_title = "Authorization service".into();
        store.put_settings(&common_project_name).await.unwrap();
        store.pool.close().await;
        let reopened = Store::connect(&url).await.unwrap();
        assert_eq!(reopened.get_settings().await.unwrap(), common_project_name);
        let mut unsafe_settings = settings.clone();
        unsafe_settings.default_title = "token=secret".into();
        assert!(reopened.put_settings(&unsafe_settings).await.is_err());
        unsafe_settings.default_title = ["sk-", "0123456789abcdef0123456789abcdef"].concat();
        assert!(reopened.put_settings(&unsafe_settings).await.is_err());
    }

    #[tokio::test]
    async fn file_database_uses_wal_and_waits_for_short_write_contention() {
        let dir = private_tempdir();
        let path = dir.path().join("contention.sqlite");
        let store = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        assert_eq!(busy_timeout, 10_000);
    }

    #[tokio::test]
    async fn file_change_planning_waits_instead_of_failing_on_a_stale_wal_snapshot() {
        let dir = private_tempdir();
        let path = dir.path().join("file-change-contention.sqlite");
        let store = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let team = scope("team_file_change_contention");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "Contention".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();

        let mut writer = store.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(Utc::now())
            .bind(&session.id.0)
            .execute(&mut *writer)
            .await
            .unwrap();
        let planning_store = store.clone();
        let planning_scope = team.clone();
        let planning_turn = turn.id.clone();
        let planning = tokio::spawn(async move {
            planning_store
                .plan_turn_file_change(
                    &planning_scope,
                    &planning_turn,
                    "src/lib.rs",
                    Some(b"before"),
                    Some(&"b".repeat(64)),
                    &"a".repeat(64),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!planning.is_finished());
        writer.commit().await.unwrap();
        let plan = tokio::time::timeout(Duration::from_secs(2), planning)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        store.complete_turn_file_change(&plan).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn database_files_are_private_and_symlinks_are_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = private_tempdir();
        let path = dir.path().join("private.sqlite");
        let url = format!("sqlite://{}", path.display());
        let store = Store::connect(&url).await.unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        store.pool.close().await;

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reopened = Store::connect(&url).await.unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        reopened.pool.close().await;

        let link = dir.path().join("database-link.sqlite");
        symlink(&path, &link).unwrap();
        let error = Store::connect(&format!("sqlite://{}", link.display()))
            .await
            .err()
            .expect("symbolic link must be rejected");
        assert!(error.to_string().contains("symbolic link"));
    }

    #[tokio::test]
    async fn database_backup_restore_is_consistent_and_rejects_corruption() {
        let dir = private_tempdir();
        let source_path = dir.path().join("source.sqlite");
        let backup_path = dir.path().join("backup.sqlite");
        let restored_path = dir.path().join("restored.sqlite");
        let source_url = format!("sqlite://{}", source_path.display());
        let store = Store::connect(&source_url).await.unwrap();
        store
            .create_session(CreateSession {
                scope: scope("team_a"),
                workspace_uri: "file:///repo".into(),
                title: "present in backup".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        store.backup_database(&backup_path).await.unwrap();
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&backup_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        store
            .create_session(CreateSession {
                scope: scope("team_a"),
                workspace_uri: "file:///repo".into(),
                title: "after backup".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        store.pool.close().await;

        Store::restore_database(&backup_path, &restored_path)
            .await
            .unwrap();
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&restored_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let restored = Store::connect(&format!("sqlite://{}", restored_path.display()))
            .await
            .unwrap();
        let sessions = restored.list_sessions(&scope("team_a")).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "present in backup");
        restored.pool.close().await;

        let corrupt = dir.path().join("corrupt.sqlite");
        std::fs::write(&corrupt, b"not a SQLite database").unwrap();
        let before = std::fs::read(&restored_path).unwrap();
        assert!(
            Store::restore_database(&corrupt, &restored_path)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&restored_path).unwrap(), before);
    }

    #[tokio::test]
    async fn published_migration_32_remains_compatible_and_upgrades_forward() {
        let full_migrator = sqlx::migrate!("./migrations");
        let current_migration_head = full_migrator
            .iter()
            .map(|migration| migration.version)
            .max()
            .expect("at least one migration must exist");
        let migration_32 = full_migrator
            .iter()
            .find(|migration| migration.version == 32)
            .expect("migration 32 must exist");
        let checksum = migration_32
            .checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            checksum,
            "b9592a3b35997e5b3f725f82428e195bfa93797cdfb7e8ca6a61285937924e6f0eaa5434ee5808642b5d7dbe43ca311c",
            "published migrations are immutable; add a new forward migration instead"
        );

        let legacy_migrator = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full_migrator
                    .iter()
                    .filter(|migration| migration.version <= 32)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        let directory = private_tempdir();
        let path = directory.path().join("migration-32.sqlite");
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        legacy_migrator.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO sessions(
                id,organization_id,team_id,actor_id,workspace_uri,title,model,status,created_at,updated_at
             ) VALUES('session-legacy','org','team','actor','file:///repo','Legacy','model','active','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO turns(
                id,session_id,organization_id,team_id,actor_id,status,started_at,updated_at
             ) VALUES('turn-legacy','session-legacy','org','team','actor','completed','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages(id,session_id,turn_id,role,content_json,created_at)
             VALUES('message-legacy','session-legacy','turn-legacy','user','\"preserved\"','2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let upgraded = Store::connect(&url).await.unwrap();
        let migration_head =
            sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(&upgraded.pool)
                .await
                .unwrap();
        assert_eq!(migration_head, current_migration_head);
        let message_count =
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages WHERE id='message-legacy'")
                .fetch_one(&upgraded.pool)
                .await
                .unwrap();
        assert_eq!(message_count, 1);
        let transcript_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM transcript_item_index
             WHERE item_id='message-legacy' AND source_kind='message'",
        )
        .fetch_one(&upgraded.pool)
        .await
        .unwrap();
        assert_eq!(transcript_count, 1);
        upgraded.verify_integrity().await.unwrap();
    }

    #[tokio::test]
    async fn migration_44_authenticates_the_existing_key_before_atomic_audit_backfill() {
        let full_migrator = sqlx::migrate!("./migrations");
        let legacy_migrator = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full_migrator
                    .iter()
                    .filter(|migration| migration.version <= 43)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        let directory = private_tempdir();
        let path = directory.path().join("migration-43.sqlite");
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        legacy_migrator.run(&pool).await.unwrap();
        let scope = scope("team_a");
        let codec = SensitiveCodec::encrypted("key-1", &[9_u8; 32]).unwrap();
        let session_id = Id("session-legacy-encrypted".into());
        let workspace = codec
            .seal_text(
                &scope,
                "sessions",
                &session_id,
                "workspace_uri",
                "file:///repo",
            )
            .unwrap();
        let title = codec
            .seal_text(&scope, "sessions", &session_id, "title", "Legacy encrypted")
            .unwrap();
        let model = codec
            .seal_text(&scope, "sessions", &session_id, "model", "model")
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions(id,organization_id,team_id,actor_id,workspace_uri,title,model,status,created_at,updated_at) \
             VALUES(?,?,?,?,?,?,?,'active',?,?)",
        )
        .bind(&session_id.0)
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(workspace)
        .bind(title)
        .bind(model)
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO audit_events(id,organization_id,team_id,actor_id,session_id,event_type,payload_json,created_at,chain_hash) \
             VALUES('legacy-event',?,?,?,?,?,?,?,?)",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(&session_id.0)
        .bind("model.delta")
        .bind(r#"{"text":"legacy-audit-canary"}"#)
        .bind(Utc::now())
        .bind("ab".repeat(32))
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let wrong = Store::connect_encrypted(&url, "key-1", &[8_u8; 32]).await;
        assert!(matches!(wrong, Err(StorageError::Encryption(_))));
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .read_only(true)
            .create_if_missing(false);
        let untouched = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let migration_head: i64 =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(&untouched)
                .await
                .unwrap();
        assert_eq!(migration_head, 43);
        untouched.close().await;

        let upgraded = Store::connect_encrypted(&url, "key-1", &[9_u8; 32])
            .await
            .unwrap();
        let raw: String =
            sqlx::query_scalar("SELECT payload_json FROM audit_events WHERE id='legacy-event'")
                .fetch_one(&upgraded.pool)
                .await
                .unwrap();
        assert!(raw.starts_with("enc:v1:key-1:"));
        assert!(!raw.contains("legacy-audit-canary"));
        let events = upgraded.list_team_events(&scope, 0, 10).await.unwrap();
        assert_eq!(events[0].payload["text"], "legacy-audit-canary");
    }

    #[tokio::test]
    async fn migration_44_authenticates_extension_only_databases_before_cutover() {
        let full_migrator = sqlx::migrate!("./migrations");
        let legacy_migrator = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full_migrator
                    .iter()
                    .filter(|migration| migration.version <= 43)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        let directory = private_tempdir();
        let path = directory.path().join("extension-only-v43.sqlite");
        let url = format!("sqlite://{}", path.display());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        legacy_migrator.run(&pool).await.unwrap();
        let team = scope("team_a");
        let server = McpServerSpec {
            id: "extension-only".into(),
            program: "/usr/bin/true".into(),
            args: Vec::new(),
            environment_handles: std::collections::BTreeMap::new(),
            timeout_ms: 1_000,
        };
        let encoded = serde_json::to_string(&server).unwrap();
        let codec = SensitiveCodec::encrypted("key-1", &[7_u8; 32]).unwrap();
        let encoded = codec
            .seal_text(
                &team,
                "mcp_installations",
                &Id("mcp:extension-only".into()),
                "spec_json",
                &encoded,
            )
            .unwrap();
        sqlx::query(
            "INSERT INTO mcp_installations \
             (organization_id,team_id,server_id,actor_id,spec_json,permissions_sha256,enabled,revision,created_at,updated_at) \
             VALUES(?,?,?,?,?,?,1,1,?,?)",
        )
        .bind(&team.organization_id.0)
        .bind(&team.team_id.0)
        .bind(&server.id)
        .bind(&team.actor_id.0)
        .bind(encoded)
        .bind("permission-digest")
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        assert!(matches!(
            Store::connect_encrypted(&url, "key-1", &[6_u8; 32]).await,
            Err(StorageError::Encryption(_))
        ));
        let untouched = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&url)
                    .unwrap()
                    .read_only(true)
                    .create_if_missing(false),
            )
            .await
            .unwrap();
        let migration_head: i64 =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
                .fetch_one(&untouched)
                .await
                .unwrap();
        let marker_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='storage_encryption_metadata'",
        )
        .fetch_one(&untouched)
        .await
        .unwrap();
        assert_eq!(migration_head, 43);
        assert_eq!(marker_exists, 0);
        untouched.close().await;

        let upgraded = Store::connect_encrypted(&url, "key-1", &[7_u8; 32])
            .await
            .unwrap();
        let installations = upgraded.list_mcp_installations(&team).await.unwrap();
        assert_eq!(installations.len(), 1);
        assert_eq!(installations[0].server, server);
    }

    #[tokio::test]
    async fn migration_failure_preserves_database_contents_in_wal_mode() {
        let dir = private_tempdir();
        let path = dir.path().join("migration.sqlite");
        let url = format!("sqlite://{}", path.display());
        let store = Store::connect(&url).await.unwrap();
        sqlx::query("CREATE TABLE migration_failure_sentinel(value TEXT NOT NULL)")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO migration_failure_sentinel(value) VALUES('preserved')")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET checksum=x'00' WHERE version=1")
            .execute(&store.pool)
            .await
            .unwrap();
        let migrations_before = sqlx::query_as::<_, (i64, String, bool, Vec<u8>, i64)>(
            "SELECT version,description,success,checksum,execution_time
             FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        let schema_before = sqlx::query_as::<_, (String, String, String)>(
            "SELECT type,name,coalesce(sql,'') FROM sqlite_schema ORDER BY type,name",
        )
        .fetch_all(&store.pool)
        .await
        .unwrap();
        store.pool.close().await;
        assert!(matches!(
            Store::connect(&url).await,
            Err(StorageError::Migration(_))
        ));

        // WAL checkpoints may legitimately move unchanged pages between the
        // sidecar and main database file. Compare the logical database state,
        // not the byte layout of only one file in that set.
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let migrations_after = sqlx::query_as::<_, (i64, String, bool, Vec<u8>, i64)>(
            "SELECT version,description,success,checksum,execution_time
             FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let schema_after = sqlx::query_as::<_, (String, String, String)>(
            "SELECT type,name,coalesce(sql,'') FROM sqlite_schema ORDER BY type,name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let sentinel =
            sqlx::query_scalar::<_, String>("SELECT value FROM migration_failure_sentinel")
                .fetch_one(&pool)
                .await
                .unwrap();
        let integrity = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        pool.close().await;

        assert_eq!(migrations_after, migrations_before);
        assert_eq!(schema_after, schema_before);
        assert_eq!(sentinel, "preserved");
        assert_eq!(integrity, ["ok"]);
    }

    #[tokio::test]
    async fn signed_central_work_snapshot_sync_is_atomic_and_preserves_local_work() {
        use opencoding_policy::{
            CentralTeamWorkPayload, ReplicatedTeamGoal, ReplicatedTeamTask, SignedTeamWorkSnapshot,
        };
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        store
            .upsert_team_capacity(UpdateTeamCapacity {
                scope: team.clone(),
                human_available_hours: 10.0,
                agent_concurrency: 2,
                wip_limit: 2,
            })
            .await
            .unwrap();
        let local = store
            .create_team_goal(CreateTeamGoal {
                scope: team.clone(),
                title: "Local goal".into(),
                outcome_definition: "Must survive sync".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let snapshot = |sequence, goals, tasks| SignedTeamWorkSnapshot {
            key_id: "verified-root".into(),
            signature: "verified-before-storage".into(),
            payload: CentralTeamWorkPayload {
                organization_id: team.organization_id.clone(),
                team_id: team.team_id.clone(),
                sequence,
                issued_at: Utc::now() - chrono::Duration::minutes(1),
                expires_at: Utc::now() + chrono::Duration::hours(1),
                goals,
                tasks,
            },
        };
        let central_goal = ReplicatedTeamGoal {
            id: Id("central-goal".into()),
            title: "Central goal".into(),
            outcome_definition: "Shared outcome".into(),
            status: GoalStatus::Active,
            target_date: None,
        };
        let central_task = ReplicatedTeamTask {
            id: Id("central-task".into()),
            goal_id: Some(central_goal.id.clone()),
            source: "control-plane".into(),
            title: "Central task".into(),
            priority: 100,
            assignee_type: Some("agent".into()),
            assignee_id: None,
            status: TeamTaskStatus::Ready,
            acceptance_criteria: vec!["done".into()],
            required_evidence: vec!["test".into()],
            blockers: vec![],
        };
        let first = snapshot(1, vec![central_goal.clone()], vec![central_task]);
        let result = store
            .apply_verified_team_work_snapshot(&first)
            .await
            .unwrap();
        assert_eq!((result.goals, result.tasks), (1, 1));
        assert_eq!(store.list_team_goals(&team).await.unwrap().len(), 2);

        let second = snapshot(2, vec![central_goal], vec![]);
        let result = store
            .apply_verified_team_work_snapshot(&second)
            .await
            .unwrap();
        assert_eq!(result.cancelled_tasks, 1);
        assert_eq!(
            store.list_team_tasks(&team).await.unwrap()[0].status,
            TeamTaskStatus::Cancelled
        );
        assert!(store.list_team_goals(&team).await.unwrap().contains(&local));
        assert!(
            store
                .apply_verified_team_work_snapshot(&second)
                .await
                .is_err()
        );

        let collision = snapshot(
            3,
            vec![ReplicatedTeamGoal {
                id: local.id.clone(),
                title: "Collision".into(),
                outcome_definition: "Must reject".into(),
                status: GoalStatus::Active,
                target_date: None,
            }],
            vec![],
        );
        assert!(
            store
                .apply_verified_team_work_snapshot(&collision)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .installed_team_work_sequence(&team.organization_id, &team.team_id)
                .await
                .unwrap(),
            Some(2)
        );
    }

    #[tokio::test]
    async fn policy_exception_matching_is_scoped_expiring_and_revocable() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let grant = |sequence, active| SignedPolicyExceptionGrant {
            key_id: "verified-root".into(),
            signature: "verified-before-storage".into(),
            payload: CentralPolicyExceptionPayload {
                organization_id: team.organization_id.clone(),
                team_id: team.team_id.clone(),
                exception_id: Id("exception-1".into()),
                sequence,
                issued_at: Utc::now() - chrono::Duration::minutes(1),
                expires_at: Utc::now() + chrono::Duration::hours(1),
                active,
                tool: "git_push".into(),
                actor_id: Some(team.actor_id.clone()),
                workspace_uri: Some("file:///repo".into()),
                reason: "release".into(),
                requested_by: team.actor_id.clone(),
                approved_by: Id("approver".into()),
            },
        };
        store
            .apply_verified_policy_exception(&grant(1, true))
            .await
            .unwrap();
        assert!(
            store
                .matching_policy_exception(&team, "git_push", "file:///repo", Utc::now())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .matching_policy_exception(&team, "git_push", "file:///other", Utc::now())
                .await
                .unwrap()
                .is_none()
        );
        let other_actor = Scope {
            actor_id: Id("other".into()),
            ..team.clone()
        };
        assert!(
            store
                .matching_policy_exception(&other_actor, "git_push", "file:///repo", Utc::now())
                .await
                .unwrap()
                .is_none()
        );
        store
            .apply_verified_policy_exception(&grant(2, false))
            .await
            .unwrap();
        assert!(
            store
                .matching_policy_exception(&team, "git_push", "file:///repo", Utc::now())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .apply_verified_policy_exception(&grant(1, true))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn sessions_are_team_scoped() {
        let store = Store::in_memory().await.unwrap();
        let team_a = scope("team_a");
        store
            .create_session(CreateSession {
                scope: team_a.clone(),
                workspace_uri: "file:///repo".into(),
                title: "A".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        assert_eq!(store.list_sessions(&team_a).await.unwrap().len(), 1);
        assert!(
            store
                .list_sessions(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );

        let mut other_organization = team_a.clone();
        other_organization.organization_id = Id("org_2".into());
        store
            .create_session(CreateSession {
                scope: other_organization.clone(),
                workspace_uri: "file:///other".into(),
                title: "Other organization".into(),
                model: "private-model".into(),
            })
            .await
            .unwrap();
        let visible = store.list_sessions(&team_a).await.unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].title, "A");
        assert_eq!(
            store
                .list_sessions(&other_organization)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn session_archive_and_delete_cancel_work_and_preserve_team_history() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "lifecycle".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        assert!(matches!(
            store
                .close_session(&scope("team_b"), &session.id, SessionStatus::Archived)
                .await,
            Err(StorageError::ScopeMismatch)
        ));

        let archived = store
            .close_session(&team, &session.id, SessionStatus::Archived)
            .await
            .unwrap();
        assert_eq!(archived.status, SessionStatus::Archived);
        assert_eq!(
            store.get_turn(&team, &turn.id).await.unwrap().status,
            TurnStatus::Cancelled
        );
        assert_eq!(store.list_sessions(&team).await.unwrap().len(), 1);
        assert!(matches!(
            store.create_turn(&team, &session.id).await,
            Err(StorageError::InvalidState(_))
        ));
        assert!(matches!(
            store
                .create_turn_with_user_message_if_idle(
                    &team,
                    &session.id,
                    serde_json::json!("late turn")
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert!(matches!(
            store
                .append_message(&team, &session.id, "user", serde_json::json!("late"))
                .await,
            Err(StorageError::InvalidState(_))
        ));

        let restored = store.restore_session(&team, &session.id).await.unwrap();
        assert_eq!(restored.status, SessionStatus::Active);
        store
            .create_turn_with_user_message_if_idle(
                &team,
                &session.id,
                serde_json::json!("after restore"),
            )
            .await
            .unwrap();

        let deleted = store
            .close_session(&team, &session.id, SessionStatus::Deleted)
            .await
            .unwrap();
        assert_eq!(deleted.status, SessionStatus::Deleted);
        assert!(matches!(
            store.get_session(&session.id).await,
            Err(StorageError::NotFound)
        ));
        assert!(store.list_sessions(&team).await.unwrap().is_empty());
        assert_eq!(
            store
                .close_session(&team, &session.id, SessionStatus::Deleted)
                .await
                .unwrap()
                .status,
            SessionStatus::Deleted
        );
    }

    #[tokio::test]
    async fn concurrent_session_close_and_turn_creation_leave_no_live_turns() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_close_race");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "close race".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(17));
        let mut attempts = Vec::new();
        for _ in 0..16 {
            let worker_store = store.clone();
            let worker_team = team.clone();
            let worker_session = session.id.clone();
            let worker_barrier = barrier.clone();
            attempts.push(tokio::spawn(async move {
                worker_barrier.wait().await;
                worker_store
                    .create_turn(&worker_team, &worker_session)
                    .await
            }));
        }
        barrier.wait().await;
        let archived = store
            .close_session(&team, &session.id, SessionStatus::Archived)
            .await
            .unwrap();
        for attempt in attempts {
            let _ = attempt.await.unwrap();
        }

        assert_eq!(archived.status, SessionStatus::Archived);
        assert!(
            store
                .list_turns(&team, &session.id)
                .await
                .unwrap()
                .iter()
                .all(|turn| matches!(
                    turn.status,
                    TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
                ))
        );
        assert!(matches!(
            store.create_turn(&team, &session.id).await,
            Err(StorageError::InvalidState(_))
        ));
    }
    #[tokio::test]
    #[cfg(unix)]
    async fn existing_database_directory_must_already_be_private() {
        let directory = private_tempdir();
        let state = directory.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
        let url = format!("sqlite://{}", state.join("opencoding.db").display());
        assert!(matches!(
            Store::connect(&url).await,
            Err(StorageError::InvalidData(message)) if message.contains("0700")
        ));
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        Store::connect(&url).await.unwrap().close().await;
    }

    #[tokio::test]
    async fn cross_team_message_is_rejected() {
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team_a"),
                workspace_uri: "file:///repo".into(),
                title: "A".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            store
                .append_message(
                    &scope("team_b"),
                    &session.id,
                    "user",
                    serde_json::json!("x")
                )
                .await,
            Err(StorageError::ScopeMismatch)
        ));
    }

    #[tokio::test]
    async fn sessions_and_personal_mcp_oauth_are_actor_private_inside_a_team() {
        let store = Store::in_memory().await.unwrap();
        let alice = scope("team_a");
        let mut bob = alice.clone();
        bob.actor_id = Id("usr_bob".into());
        let session = store
            .create_session(CreateSession {
                scope: alice.clone(),
                workspace_uri: "file:///private/alice".into(),
                title: "Alice private session".into(),
                model: "private-model".into(),
            })
            .await
            .unwrap();
        store
            .append_message(
                &alice,
                &session.id,
                "user",
                serde_json::json!("private prompt"),
            )
            .await
            .unwrap();
        let turn = store.create_turn(&alice, &session.id).await.unwrap();
        assert!(store.list_sessions(&bob).await.unwrap().is_empty());
        assert!(matches!(
            store.list_messages(&bob, &session.id).await,
            Err(StorageError::ScopeMismatch)
        ));
        for denied in [
            store.list_turns(&bob, &session.id).await.map(|_| ()),
            store.create_turn(&bob, &session.id).await.map(|_| ()),
            store.session_usage(&bob, &session.id).await.map(|_| ()),
            store.get_turn(&bob, &turn.id).await.map(|_| ()),
            store
                .close_session(&bob, &session.id, SessionStatus::Archived)
                .await
                .map(|_| ()),
        ] {
            assert!(matches!(denied, Err(StorageError::ScopeMismatch)));
        }
        assert!(matches!(
            store
                .update_session_title(&bob, &session.id, "stolen")
                .await,
            Err(StorageError::ScopeMismatch)
        ));

        let goal = store
            .create_team_goal(CreateTeamGoal {
                scope: alice.clone(),
                title: "Shared goal".into(),
                outcome_definition: "Alice and Bob can collaborate".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let task = store
            .create_team_task(CreateTeamTask {
                scope: alice.clone(),
                goal_id: Some(goal.id.clone()),
                source: "local".into(),
                title: "Shared task".into(),
                priority: 10,
                assignee_type: None,
                assignee_id: None,
                acceptance_criteria: vec!["done".into()],
                required_evidence: vec![],
            })
            .await
            .unwrap();
        assert!(store.list_team_goals(&bob).await.unwrap().contains(&goal));
        assert!(store.list_team_tasks(&bob).await.unwrap().contains(&task));
        let claimed = store
            .update_team_task(
                &task.id,
                UpdateTeamTask {
                    scope: bob.clone(),
                    status: TeamTaskStatus::InProgress,
                    assignee_type: Some("human".into()),
                    assignee_id: Some(bob.actor_id.clone()),
                    blockers: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert_eq!(claimed.assignee_id, Some(bob.actor_id.clone()));
        let durable = store
            .create_durable_task(CreateDurableTask {
                scope: alice.clone(),
                kind: "team.shared".into(),
                payload: serde_json::json!({"work":"shared"}),
                idempotency_key: "shared-across-actors".into(),
                max_attempts: 1,
                max_runtime_seconds: 60,
                max_cost_micros: 0,
                max_runner_cost_micros: 0,
            })
            .await
            .unwrap();
        assert_eq!(
            store.get_durable_task(&bob, &durable.id).await.unwrap().id,
            durable.id
        );

        let now = Utc::now();
        let credential = |scope: &Scope, token: &str| McpOAuthCredential {
            scope: scope.clone(),
            server_id: "personal-server".into(),
            access_token: token.into(),
            refresh_token: Some(format!("refresh-{token}")),
            token_type: "Bearer".into(),
            scopes: vec!["read".into()],
            expires_at: None,
            token_endpoint: "https://mcp.example.invalid/token".into(),
            revocation_endpoint: None,
            client_id: "community-client".into(),
            created_at: now,
            updated_at: now,
        };
        store
            .put_mcp_oauth_credential(&alice, credential(&alice, "alice-token"))
            .await
            .unwrap();
        assert!(matches!(
            store
                .get_mcp_oauth_credential(&bob, "personal-server")
                .await,
            Err(StorageError::NotFound)
        ));
        store
            .put_mcp_oauth_credential(&bob, credential(&bob, "bob-token"))
            .await
            .unwrap();
        assert_eq!(
            store
                .get_mcp_oauth_credential(&alice, "personal-server")
                .await
                .unwrap()
                .access_token,
            "alice-token"
        );
        assert_eq!(
            store
                .remove_mcp_oauth_credential(&bob, "personal-server")
                .await
                .unwrap()
                .unwrap()
                .access_token,
            "bob-token"
        );
        assert_eq!(
            store
                .get_mcp_oauth_credential(&alice, "personal-server")
                .await
                .unwrap()
                .access_token,
            "alice-token"
        );
    }

    #[tokio::test]
    async fn turns_checkpoint_and_stop_at_terminal_state() {
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team_a"),
                workspace_uri: "file:///repo".into(),
                title: "A".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let turn = store
            .create_turn(&scope("team_a"), &session.id)
            .await
            .unwrap();
        store
            .append_turn_message(
                &scope("team_a"),
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("hello"),
            )
            .await
            .unwrap();
        let checkpoint = serde_json::json!({"model_calls": 1});
        let completed = store
            .update_turn(
                &scope("team_a"),
                &turn.id,
                TurnStatus::Completed,
                Some(&checkpoint),
                None,
            )
            .await
            .unwrap();
        assert_eq!(completed.checkpoint, Some(checkpoint));
        assert!(completed.completed_at.is_some());
        assert!(matches!(
            store
                .update_turn(
                    &scope("team_a"),
                    &turn.id,
                    TurnStatus::CallingModel,
                    None,
                    None
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(
            store
                .list_messages(&scope("team_a"), &session.id)
                .await
                .unwrap()[0]
                .turn_id,
            turn.id
        );
        assert!(matches!(
            store.get_turn(&scope("team_b"), &turn.id).await,
            Err(StorageError::ScopeMismatch)
        ));
    }

    #[tokio::test]
    async fn context_compaction_checkpoint_uses_source_compare_and_swap() {
        let team = scope("team_compaction");
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "Compaction".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let first = store
            .append_turn_message(
                &team,
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("first"),
            )
            .await
            .unwrap();
        store
            .update_turn(&team, &turn.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();
        let second = store
            .append_turn_message(
                &team,
                &session.id,
                &turn.id,
                "assistant",
                serde_json::json!("arrived during preparation"),
            )
            .await
            .unwrap();

        assert!(matches!(
            store
                .append_context_compaction(
                    &team,
                    &session.id,
                    &turn.id,
                    1,
                    Some(&first.id),
                    serde_json::json!({"schema_version":1}),
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        let marker = store
            .append_context_compaction(
                &team,
                &session.id,
                &turn.id,
                2,
                Some(&second.id),
                serde_json::json!({"schema_version":1}),
            )
            .await
            .unwrap();
        assert_eq!(marker.role, "context_compaction_state");
        assert_eq!(
            store
                .list_messages(&team, &session.id)
                .await
                .unwrap()
                .iter()
                .filter(|message| message.role == "context_compaction_state")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn context_state_waits_for_a_concurrent_writer_before_taking_its_snapshot() {
        let directory = private_tempdir();
        let database = directory.path().join("context-state-race.sqlite");
        let store = Store::connect(&format!("sqlite://{}", database.display()))
            .await
            .unwrap();
        let team = scope("team_context_state_race");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "Before title generation".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let message = store
            .append_turn_message(
                &team,
                &session.id,
                &turn.id,
                "user",
                serde_json::json!("undo this"),
            )
            .await
            .unwrap();
        store
            .update_turn(&team, &turn.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();

        // Model the asynchronous first-turn title writer that can overlap an
        // immediate /undo request after the terminal Turn event is visible.
        let mut title_writer = store.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query("UPDATE sessions SET title=? WHERE id=?")
            .bind("Generated title")
            .bind(&session.id.0)
            .execute(&mut *title_writer)
            .await
            .unwrap();

        let pending_store = store.clone();
        let pending_team = team.clone();
        let pending_session = session.id.clone();
        let pending_turn = turn.id.clone();
        let pending_message = message.id.clone();
        let pending = tokio::spawn(async move {
            pending_store
                .append_conversation_undo(
                    &pending_team,
                    &pending_session,
                    &pending_turn,
                    1,
                    Some(&pending_message),
                    serde_json::json!({"schema_version":1}),
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !pending.is_finished(),
            "context-state write must wait instead of failing its deferred transaction upgrade"
        );

        title_writer.commit().await.unwrap();
        let marker = pending.await.unwrap().unwrap();
        assert_eq!(marker.role, "conversation_undo_state");
    }

    fn durable_input(team: &str, key: &str) -> CreateDurableTask {
        CreateDurableTask {
            scope: scope(team),
            kind: "agent.turn".into(),
            payload: serde_json::json!({"prompt":"finish the task"}),
            idempotency_key: key.into(),
            max_attempts: 2,
            max_runtime_seconds: 300,
            max_cost_micros: 1_000,
            max_runner_cost_micros: 0,
        }
    }

    #[tokio::test]
    async fn durable_tasks_are_idempotent_leased_and_checkpointed() {
        let store = Store::in_memory().await.unwrap();
        let created = store
            .create_durable_task(durable_input("team_a", "key-1"))
            .await
            .unwrap();
        let repeated = store
            .create_durable_task(durable_input("team_a", "key-1"))
            .await
            .unwrap();
        assert_eq!(created.id, repeated.id);
        let leased = store
            .lease_durable_task("worker-a", 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased.status, DurableTaskStatus::Leased);
        assert_eq!(leased.attempt, 1);
        assert!(
            store
                .lease_durable_task("worker-b", 30)
                .await
                .unwrap()
                .is_none()
        );
        let token = leased.lease_token.clone().unwrap();
        let running = store.start_durable_task(&leased.id, &token).await.unwrap();
        assert_eq!(running.status, DurableTaskStatus::Running);
        let checkpointed = store
            .checkpoint_durable_task(&leased.id, &token, &serde_json::json!({"step":2}), 100)
            .await
            .unwrap();
        assert_eq!(checkpointed.checkpoint, Some(serde_json::json!({"step":2})));
        let completed = store
            .complete_durable_task(&leased.id, &token, &serde_json::json!({"ok":true}), 200)
            .await
            .unwrap();
        assert_eq!(completed.status, DurableTaskStatus::Succeeded);
        assert!(completed.completed_at.is_some());
    }

    #[tokio::test]
    async fn durable_cancellation_is_requested_cross_process_and_acknowledged_by_lease_owner() {
        let store = Store::in_memory().await.unwrap();
        let queued = store
            .create_durable_task(durable_input("team_a", "cancel-queued"))
            .await
            .unwrap();
        let queued = store
            .request_durable_task_cancellation(&scope("team_a"), &queued.id)
            .await
            .unwrap();
        assert_eq!(queued.status, DurableTaskStatus::Cancelled);
        assert!(queued.cancel_requested);
        assert!(queued.completed_at.is_some());

        let active = store
            .create_durable_task(durable_input("team_a", "cancel-active"))
            .await
            .unwrap();
        assert!(matches!(
            store
                .request_durable_task_cancellation(&scope("team_b"), &active.id)
                .await,
            Err(StorageError::ScopeMismatch)
        ));
        let leased = store
            .lease_durable_task("worker-a", 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased.id, active.id);
        let token = leased.lease_token.clone().unwrap();
        store.start_durable_task(&leased.id, &token).await.unwrap();
        let requested = store
            .request_durable_task_cancellation(&scope("team_a"), &leased.id)
            .await
            .unwrap();
        assert_eq!(requested.status, DurableTaskStatus::Running);
        assert!(requested.cancel_requested);
        assert_eq!(requested.lease_token.as_deref(), Some(token.as_str()));
        assert!(matches!(
            store
                .checkpoint_durable_task(&leased.id, &token, &serde_json::json!({}), 0)
                .await,
            Err(StorageError::InvalidState(_))
        ));
        let cancelled = store
            .acknowledge_durable_task_cancellation(
                &leased.id,
                &token,
                &serde_json::json!({"status":"cancelled"}),
                25,
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status, DurableTaskStatus::Cancelled);
        assert_eq!(
            cancelled.result,
            Some(serde_json::json!({"status":"cancelled"}))
        );
        assert_eq!(cancelled.consumed_cost_micros, 25);
        assert!(cancelled.lease_token.is_none());
    }

    #[tokio::test]
    async fn durable_workers_lease_only_their_task_kind_and_can_renew() {
        let store = Store::in_memory().await.unwrap();
        let agent = store
            .create_durable_task(durable_input("team_a", "agent-kind"))
            .await
            .unwrap();
        let mut runner_input = durable_input("team_a", "runner-kind");
        runner_input.kind = opencoding_protocol::LINUX_RUNNER_TASK_KIND.into();
        let runner = store.create_durable_task(runner_input).await.unwrap();

        let leased = store
            .lease_durable_task_kind(
                "linux-runner",
                30,
                Some(opencoding_protocol::LINUX_RUNNER_TASK_KIND),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased.id, runner.id);
        let token = leased.lease_token.as_deref().unwrap();
        let renewed = store
            .renew_durable_task_lease(&runner.id, token, 60)
            .await
            .unwrap();
        assert!(renewed.lease_expires_at > leased.lease_expires_at);
        store
            .complete_durable_task(&runner.id, token, &serde_json::json!({"ok":true}), 0)
            .await
            .unwrap();

        let leased_agent = store
            .lease_durable_task_kind("agent-worker", 30, Some("agent.turn"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leased_agent.id, agent.id);
    }

    #[tokio::test]
    async fn durable_task_retry_budget_and_stale_lease_fail_closed() {
        let store = Store::in_memory().await.unwrap();
        store
            .create_durable_task(durable_input("team_a", "key-2"))
            .await
            .unwrap();
        let first = store
            .lease_durable_task("worker-a", 30)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            store
                .checkpoint_durable_task(&first.id, "wrong", &serde_json::json!({}), 0)
                .await,
            Err(StorageError::InvalidState(_))
        ));
        let first = store
            .fail_durable_task(
                &first.id,
                first.lease_token.as_deref().unwrap(),
                "temporary",
                true,
                200,
            )
            .await
            .unwrap();
        assert_eq!(first.status, DurableTaskStatus::Queued);
        let second = store
            .lease_durable_task("worker-b", 30)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.attempt, 2);
        assert!(matches!(
            store
                .complete_durable_task(
                    &second.id,
                    second.lease_token.as_deref().unwrap(),
                    &serde_json::json!({}),
                    1_001
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        let failed = store
            .fail_durable_task(
                &second.id,
                second.lease_token.as_deref().unwrap(),
                "again",
                true,
                300,
            )
            .await
            .unwrap();
        assert_eq!(failed.status, DurableTaskStatus::Failed);
    }

    #[tokio::test]
    async fn team_kill_switch_revokes_active_durable_work() {
        let store = Store::in_memory().await.unwrap();
        let task = store
            .create_durable_task(durable_input("team_a", "key-3"))
            .await
            .unwrap();
        let leased = store
            .lease_durable_task("worker", 30)
            .await
            .unwrap()
            .unwrap();
        store
            .set_team_durable_tasks_enabled(&scope("team_a"), false)
            .await
            .unwrap();
        let cancelled = store
            .get_durable_task(&scope("team_a"), &task.id)
            .await
            .unwrap();
        assert_eq!(cancelled.status, DurableTaskStatus::Cancelled);
        assert!(matches!(
            store
                .start_durable_task(&leased.id, leased.lease_token.as_deref().unwrap())
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert!(
            store
                .lease_durable_task("worker", 30)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn editor_context_is_versioned_bounded_and_team_scoped() {
        let store = Store::in_memory().await.unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope("team_a"),
                workspace_uri: "file:///repo/".into(),
                title: "IDE".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let input = UpdateEditorContext {
            scope: scope("team_a"),
            protocol_version: opencoding_protocol::IDE_PROTOCOL_VERSION.into(),
            client_instance_id: Id("ide_1".into()),
            workspace_uri: "file:///repo/".into(),
            active_document: Some(opencoding_protocol::EditorDocument {
                uri: "file:///repo/src/lib.rs".into(),
                language_id: "rust".into(),
                version: 7,
                text: Some("fn main() {}".into()),
            }),
            selection: None,
            diagnostics: vec![],
        };
        store
            .upsert_editor_context(&session.id, input)
            .await
            .unwrap();
        let latest = store
            .latest_editor_context(&scope("team_a"), &session.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.active_document.unwrap().version, 7);
        assert!(matches!(
            store
                .latest_editor_context(&scope("team_b"), &session.id)
                .await,
            Err(StorageError::ScopeMismatch)
        ));
    }

    #[tokio::test]
    async fn team_work_is_verified_only_with_required_evidence() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let goal = store
            .create_team_goal(CreateTeamGoal {
                scope: team.clone(),
                title: "Ship reliable feature".into(),
                outcome_definition: "PR merged after required checks".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let task = store
            .create_team_task(CreateTeamTask {
                scope: team.clone(),
                goal_id: Some(goal.id.clone()),
                source: "github-issue:1".into(),
                title: "Implement feature".into(),
                priority: 100,
                assignee_type: Some("agent".into()),
                assignee_id: Some(Id("agent_1".into())),
                acceptance_criteria: vec!["behavior works".into()],
                required_evidence: vec!["tests".into(), "review".into()],
            })
            .await
            .unwrap();
        assert!(matches!(
            store
                .update_team_task(
                    &task.id,
                    UpdateTeamTask {
                        scope: team.clone(),
                        status: TeamTaskStatus::Verified,
                        assignee_type: task.assignee_type.clone(),
                        assignee_id: task.assignee_id.clone(),
                        blockers: vec![],
                    }
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        for status in [TeamTaskStatus::InProgress, TeamTaskStatus::Review] {
            store
                .update_team_task(
                    &task.id,
                    UpdateTeamTask {
                        scope: team.clone(),
                        status,
                        assignee_type: task.assignee_type.clone(),
                        assignee_id: task.assignee_id.clone(),
                        blockers: vec![],
                    },
                )
                .await
                .unwrap();
        }
        let evidence = |kind: &str| OutcomeEvidence {
            kind: kind.into(),
            uri: format!("evidence://{kind}"),
            result: "passed".into(),
            collected_at: Utc::now(),
        };
        let goal_id = goal.id.clone();
        assert!(matches!(
            store
                .update_team_goal(
                    &goal_id,
                    UpdateTeamGoal {
                        scope: team.clone(),
                        title: goal.title.clone(),
                        outcome_definition: goal.outcome_definition.clone(),
                        status: GoalStatus::Achieved,
                        target_date: goal.target_date,
                    },
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert!(matches!(
            store
                .create_team_outcome(CreateTeamOutcome {
                    scope: team.clone(),
                    goal_id: goal.id.clone(),
                    task_id: task.id.clone(),
                    evidence: vec![evidence("tests")],
                    pull_request_url: Some("https://example/pr/1".into()),
                })
                .await,
            Err(StorageError::InvalidState(_))
        ));
        store
            .create_team_outcome(CreateTeamOutcome {
                scope: team.clone(),
                goal_id: goal.id,
                task_id: task.id,
                evidence: vec![evidence("tests"), evidence("review")],
                pull_request_url: Some("https://example/pr/1".into()),
            })
            .await
            .unwrap();
        let dashboard = store.team_dashboard(&team).await.unwrap();
        assert_eq!(dashboard.verified_outcomes, 1);
        assert_eq!(dashboard.review_tasks, 0);
        assert_eq!(
            store.list_team_tasks(&team).await.unwrap()[0].status,
            TeamTaskStatus::Verified
        );
        let achieved = store
            .update_team_goal(
                &goal_id,
                UpdateTeamGoal {
                    scope: team.clone(),
                    title: goal.title.clone(),
                    outcome_definition: goal.outcome_definition.clone(),
                    status: GoalStatus::Achieved,
                    target_date: goal.target_date,
                },
            )
            .await
            .unwrap();
        assert_eq!(achieved.status, GoalStatus::Achieved);
        assert!(matches!(
            store
                .update_team_goal(
                    &goal_id,
                    UpdateTeamGoal {
                        scope: team,
                        title: goal.title,
                        outcome_definition: goal.outcome_definition,
                        status: GoalStatus::Active,
                        target_date: goal.target_date,
                    },
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
    }

    #[tokio::test]
    async fn concurrent_team_task_cancellation_cannot_be_resurrected() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_concurrency");
        let goal = store
            .create_team_goal(CreateTeamGoal {
                scope: team.clone(),
                title: "Race-safe goal".into(),
                outcome_definition: "Terminal state remains terminal".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let make_task = |title: &str| CreateTeamTask {
            scope: team.clone(),
            goal_id: Some(goal.id.clone()),
            source: "test".into(),
            title: title.into(),
            priority: 1,
            assignee_type: None,
            assignee_id: None,
            acceptance_criteria: Vec::new(),
            required_evidence: vec!["tests".into()],
        };
        let task = store
            .create_team_task(make_task("Update race"))
            .await
            .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let (cancelled, started) = tokio::join!(
            async {
                barrier.wait().await;
                store
                    .update_team_task(
                        &task.id,
                        UpdateTeamTask {
                            scope: team.clone(),
                            status: TeamTaskStatus::Cancelled,
                            assignee_type: None,
                            assignee_id: None,
                            blockers: Vec::new(),
                        },
                    )
                    .await
            },
            async {
                barrier.wait().await;
                store
                    .update_team_task(
                        &task.id,
                        UpdateTeamTask {
                            scope: team.clone(),
                            status: TeamTaskStatus::InProgress,
                            assignee_type: Some("agent".into()),
                            assignee_id: Some(Id("agent".into())),
                            blockers: Vec::new(),
                        },
                    )
                    .await
            }
        );
        assert!(cancelled.is_ok() || started.is_ok());
        let final_status = store.get_team_task(&team, &task.id).await.unwrap().status;
        if cancelled.is_ok() {
            // Starting may commit first and then be superseded by cancellation. Both
            // callers can truthfully report success, but cancellation must remain
            // terminal and can never be overwritten by the racing start.
            assert_eq!(final_status, TeamTaskStatus::Cancelled);
        } else {
            assert_eq!(final_status, TeamTaskStatus::InProgress);
        }

        let review_task = store
            .create_team_task(make_task("Outcome race"))
            .await
            .unwrap();
        for status in [TeamTaskStatus::InProgress, TeamTaskStatus::Review] {
            store
                .update_team_task(
                    &review_task.id,
                    UpdateTeamTask {
                        scope: team.clone(),
                        status,
                        assignee_type: None,
                        assignee_id: None,
                        blockers: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let (cancelled, verified) = tokio::join!(
            async {
                barrier.wait().await;
                store
                    .update_team_task(
                        &review_task.id,
                        UpdateTeamTask {
                            scope: team.clone(),
                            status: TeamTaskStatus::Cancelled,
                            assignee_type: None,
                            assignee_id: None,
                            blockers: Vec::new(),
                        },
                    )
                    .await
            },
            async {
                barrier.wait().await;
                store
                    .create_team_outcome(CreateTeamOutcome {
                        scope: team.clone(),
                        goal_id: goal.id.clone(),
                        task_id: review_task.id.clone(),
                        evidence: vec![OutcomeEvidence {
                            kind: "tests".into(),
                            uri: "evidence://tests".into(),
                            result: "passed".into(),
                            collected_at: Utc::now(),
                        }],
                        pull_request_url: None,
                    })
                    .await
            }
        );
        assert_ne!(cancelled.is_ok(), verified.is_ok());
        let final_status = store
            .get_team_task(&team, &review_task.id)
            .await
            .unwrap()
            .status;
        assert_eq!(
            final_status,
            if verified.is_ok() {
                TeamTaskStatus::Verified
            } else {
                TeamTaskStatus::Cancelled
            }
        );

        let cancelled_goal = store
            .update_team_goal(
                &goal.id,
                UpdateTeamGoal {
                    scope: team.clone(),
                    title: goal.title.clone(),
                    outcome_definition: goal.outcome_definition.clone(),
                    status: GoalStatus::Cancelled,
                    target_date: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(cancelled_goal.status, GoalStatus::Cancelled);
        assert!(store.create_team_task(make_task("Too late")).await.is_err());
        assert!(
            store
                .list_team_tasks(&team)
                .await
                .unwrap()
                .into_iter()
                .all(|task| matches!(
                    task.status,
                    TeamTaskStatus::Verified | TeamTaskStatus::Cancelled
                ))
        );
    }

    #[tokio::test]
    async fn knowledge_pack_excludes_expired_and_cross_team_items() {
        let store = Store::in_memory().await.unwrap();
        for (team, version, valid_until) in [
            (
                "team_a",
                "current",
                Some(Utc::now() + chrono::Duration::days(1)),
            ),
            (
                "team_a",
                "expired",
                Some(Utc::now() - chrono::Duration::days(1)),
            ),
            ("team_b", "other", None),
        ] {
            store
                .create_team_knowledge(CreateTeamKnowledgeItem {
                    scope: scope(team),
                    source_uri: format!("knowledge://{version}"),
                    title: version.into(),
                    content: "runbook".into(),
                    version: version.into(),
                    trust_level: "reviewed".into(),
                    permission: "team".into(),
                    valid_until,
                })
                .await
                .unwrap();
        }
        store
            .create_team_knowledge(CreateTeamKnowledgeItem {
                scope: scope("team_a"),
                source_uri: "knowledge://private".into(),
                title: "private".into(),
                content: "restricted runbook".into(),
                version: "current".into(),
                trust_level: "reviewed".into(),
                permission: "actor:usr_2".into(),
                valid_until: None,
            })
            .await
            .unwrap();
        let active = store
            .list_team_knowledge(&scope("team_a"), false)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].version, "current");
        let mut other_actor = scope("team_a");
        other_actor.actor_id = Id("usr_2".into());
        let other_active = store
            .list_team_knowledge(&other_actor, false)
            .await
            .unwrap();
        assert_eq!(other_active.len(), 2);
        assert!(other_active.iter().any(|item| item.title == "private"));
        assert!(matches!(
            store
                .create_team_knowledge(CreateTeamKnowledgeItem {
                    scope: scope("team_a"),
                    source_uri: "knowledge://invalid".into(),
                    title: "invalid".into(),
                    content: "must fail closed".into(),
                    version: "current".into(),
                    trust_level: "reviewed".into(),
                    permission: "everyone".into(),
                    valid_until: None,
                })
                .await,
            Err(StorageError::InvalidData(_))
        ));
        let dashboard = store.team_dashboard(&scope("team_a")).await.unwrap();
        assert_eq!(dashboard.knowledge_items, 3);
        assert_eq!(dashboard.stale_knowledge_items, 1);
    }

    #[tokio::test]
    async fn team_resources_are_scoped_and_capacity_is_upserted() {
        let store = Store::in_memory().await.unwrap();
        let ownership = store
            .create_team_ownership(CreateTeamOwnership {
                scope: scope("team_a"),
                resource_type: "repository".into(),
                resource_uri: "https://github.example/acme/api".into(),
                service_tier: Some("tier-1".into()),
                on_call: Some("pagerduty://api".into()),
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .list_team_ownership(&scope("team_a"))
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .list_team_ownership(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .create_team_ownership(CreateTeamOwnership {
                    scope: scope("team_b"),
                    resource_type: "repository".into(),
                    resource_uri: "https://github.example/acme/api".into(),
                    service_tier: None,
                    on_call: None,
                })
                .await
                .is_err()
        );
        let transferred = store
            .update_team_ownership(
                &ownership.id,
                UpdateTeamOwnership {
                    scope: scope("team_a"),
                    service_tier: Some("tier-0".into()),
                    on_call: Some("pagerduty://platform".into()),
                    new_team_id: Some(Id("team_b".into())),
                    archived: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(transferred.scope.team_id.0, "team_b");
        assert!(
            store
                .list_team_ownership(&scope("team_a"))
                .await
                .unwrap()
                .is_empty()
        );
        store
            .update_team_ownership(
                &ownership.id,
                UpdateTeamOwnership {
                    scope: scope("team_b"),
                    service_tier: transferred.service_tier,
                    on_call: transferred.on_call,
                    new_team_id: None,
                    archived: true,
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .list_team_ownership(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );

        for hours in [40.0, 24.5] {
            store
                .upsert_team_capacity(UpdateTeamCapacity {
                    scope: scope("team_a"),
                    human_available_hours: hours,
                    agent_concurrency: 4,
                    wip_limit: 6,
                })
                .await
                .unwrap();
        }
        let capacity = store.get_team_capacity(&scope("team_a")).await.unwrap();
        assert_eq!(capacity.human_available_hours, 24.5);
        assert_eq!(capacity.wip_limit, 6);
        assert!(matches!(
            store.get_team_capacity(&scope("team_b")).await,
            Err(StorageError::NotFound)
        ));
    }

    #[tokio::test]
    async fn team_wip_and_agent_concurrency_are_enforced_at_admission() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        store
            .upsert_team_capacity(UpdateTeamCapacity {
                scope: team.clone(),
                human_available_hours: 8.0,
                agent_concurrency: 1,
                wip_limit: 1,
            })
            .await
            .unwrap();
        let mut tasks = Vec::new();
        for title in ["first", "second"] {
            tasks.push(
                store
                    .create_team_task(CreateTeamTask {
                        scope: team.clone(),
                        goal_id: None,
                        source: format!("test://{title}"),
                        title: title.into(),
                        priority: 1,
                        assignee_type: Some("agent".into()),
                        assignee_id: Some(Id("agent_1".into())),
                        acceptance_criteria: vec![],
                        required_evidence: vec![],
                    })
                    .await
                    .unwrap(),
            );
        }
        let update = |task: &TeamTask| UpdateTeamTask {
            scope: team.clone(),
            status: TeamTaskStatus::InProgress,
            assignee_type: task.assignee_type.clone(),
            assignee_id: task.assignee_id.clone(),
            blockers: vec![],
        };
        store
            .update_team_task(&tasks[0].id, update(&tasks[0]))
            .await
            .unwrap();
        assert!(matches!(
            store
                .update_team_task(&tasks[1].id, update(&tasks[1]))
                .await,
            Err(StorageError::InvalidState(_))
        ));

        store
            .create_durable_task(durable_input("team_a", "capacity-1"))
            .await
            .unwrap();
        store
            .create_durable_task(durable_input("team_a", "capacity-2"))
            .await
            .unwrap();
        let first = store
            .lease_durable_task("worker-1", 30)
            .await
            .unwrap()
            .unwrap();
        assert!(
            store
                .lease_durable_task("worker-2", 30)
                .await
                .unwrap()
                .is_none()
        );
        store
            .complete_durable_task(
                &first.id,
                first.lease_token.as_deref().unwrap(),
                &serde_json::json!({"ok":true}),
                0,
            )
            .await
            .unwrap();
        assert!(
            store
                .lease_durable_task("worker-2", 30)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn team_budget_consumption_is_atomic_idempotent_and_fail_closed() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        store
            .create_team_budget(CreateTeamBudget {
                scope: team.clone(),
                period_start: Utc::now() - chrono::Duration::hours(1),
                period_end: Utc::now() + chrono::Duration::hours(1),
                model_limit_micros: 100,
                runner_limit_micros: 50,
                hard_limit: true,
            })
            .await
            .unwrap();
        let consume = ConsumeTeamBudget {
            scope: team.clone(),
            model_micros: 60,
            runner_micros: 20,
            idempotency_key: "turn-1".into(),
        };
        let first = store.consume_team_budget(consume.clone()).await.unwrap();
        let repeated = store.consume_team_budget(consume).await.unwrap();
        assert_eq!(first.model_consumed_micros, 60);
        assert_eq!(repeated.model_consumed_micros, 60);
        assert!(matches!(
            store
                .consume_team_budget(ConsumeTeamBudget {
                    scope: team.clone(),
                    model_micros: 41,
                    runner_micros: 1,
                    idempotency_key: "turn-2".into(),
                })
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(
            store.list_team_budgets(&team).await.unwrap()[0].model_consumed_micros,
            60
        );
        assert!(matches!(
            store
                .consume_team_budget(ConsumeTeamBudget {
                    scope: scope("team_b"),
                    model_micros: 1,
                    runner_micros: 1,
                    idempotency_key: "cross-team".into(),
                })
                .await,
            Err(StorageError::NotFound)
        ));
    }

    #[tokio::test]
    async fn durable_cost_is_reserved_settled_retried_and_released_atomically() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        store
            .create_team_budget(CreateTeamBudget {
                scope: team.clone(),
                period_start: Utc::now() - chrono::Duration::hours(1),
                period_end: Utc::now() + chrono::Duration::hours(1),
                model_limit_micros: 100,
                runner_limit_micros: 100,
                hard_limit: true,
            })
            .await
            .unwrap();
        let mut input = durable_input("team_a", "reserved-1");
        input.max_cost_micros = 70;
        input.max_runner_cost_micros = 60;
        let task = store.create_durable_task(input.clone()).await.unwrap();
        store.create_durable_task(input).await.unwrap();
        let budget = &store.list_team_budgets(&team).await.unwrap()[0];
        assert_eq!(budget.model_reserved_micros, 70);
        assert_eq!(budget.model_consumed_micros, 0);
        assert_eq!(budget.runner_reserved_micros, 60);

        let mut rejected = durable_input("team_a", "reserved-2");
        rejected.max_cost_micros = 31;
        assert!(matches!(
            store.create_durable_task(rejected).await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(store.list_durable_tasks(&team).await.unwrap().len(), 1);

        let leased = store
            .lease_durable_task("worker", 30)
            .await
            .unwrap()
            .unwrap();
        let token = leased.lease_token.as_deref().unwrap();
        store
            .checkpoint_durable_task_costs(&task.id, token, &serde_json::json!({"step":1}), 30, 20)
            .await
            .unwrap();
        let budget = &store.list_team_budgets(&team).await.unwrap()[0];
        assert_eq!(budget.model_consumed_micros, 30);
        assert_eq!(budget.model_reserved_micros, 40);
        assert_eq!(budget.runner_consumed_micros, 20);
        assert_eq!(budget.runner_reserved_micros, 40);
        store
            .fail_durable_task_costs(&task.id, token, "retry", true, 40, 30)
            .await
            .unwrap();
        let budget = &store.list_team_budgets(&team).await.unwrap()[0];
        assert_eq!(budget.model_consumed_micros, 40);
        assert_eq!(budget.model_reserved_micros, 30);
        assert_eq!(budget.runner_consumed_micros, 30);
        assert_eq!(budget.runner_reserved_micros, 30);

        let leased = store
            .lease_durable_task("worker", 30)
            .await
            .unwrap()
            .unwrap();
        store
            .complete_durable_task_costs(
                &task.id,
                leased.lease_token.as_deref().unwrap(),
                &serde_json::json!({"ok":true}),
                50,
                40,
            )
            .await
            .unwrap();
        let budget = &store.list_team_budgets(&team).await.unwrap()[0];
        assert_eq!(budget.model_consumed_micros, 50);
        assert_eq!(budget.model_reserved_micros, 0);
        assert_eq!(budget.runner_consumed_micros, 40);
        assert_eq!(budget.runner_reserved_micros, 0);

        let mut cancelled = durable_input("team_a", "reserved-cancelled");
        cancelled.max_cost_micros = 40;
        cancelled.max_runner_cost_micros = 20;
        let cancelled = store.create_durable_task(cancelled).await.unwrap();
        assert_eq!(
            store.list_team_budgets(&team).await.unwrap()[0].model_reserved_micros,
            40
        );
        assert_eq!(
            store.list_team_budgets(&team).await.unwrap()[0].runner_reserved_micros,
            20
        );
        store
            .set_durable_task_status(&team, &cancelled.id, DurableTaskStatus::Cancelled)
            .await
            .unwrap();
        assert_eq!(
            store.list_team_budgets(&team).await.unwrap()[0].model_reserved_micros,
            0
        );
        assert_eq!(
            store.list_team_budgets(&team).await.unwrap()[0].runner_reserved_micros,
            0
        );
    }

    #[tokio::test]
    async fn audit_events_are_database_enforced_append_only() {
        let store = Store::in_memory().await.unwrap();
        let event = Event {
            id: Id("event-1".into()),
            sequence: 1,
            timestamp: Utc::now(),
            scope: scope("team_a"),
            session_id: None,
            turn_id: None,
            kind: "policy.decided".into(),
            payload: serde_json::json!({"decision":"deny"}),
        };
        store.append_event(&event, &"ab".repeat(32)).await.unwrap();
        assert_eq!(
            store.latest_event_chain_hash().await.unwrap().as_deref(),
            Some("abababababababababababababababababababababababababababababababab")
        );
        assert!(
            sqlx::query("UPDATE audit_events SET event_type='tampered' WHERE sequence=1")
                .execute(&store.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM audit_events WHERE sequence=1")
                .execute(&store.pool)
                .await
                .is_err()
        );
        assert_eq!(
            store
                .list_events(&Id("team_a".into()), 0, 10)
                .await
                .unwrap()[0]
                .kind,
            "policy.decided"
        );
    }

    #[tokio::test]
    async fn central_audit_export_cursor_is_scope_bound_monotonic_and_idempotent() {
        let store = Store::in_memory().await.unwrap();
        let source = Id("source-a".into());
        let organization = Id("org-a".into());
        let team = Id("team-a".into());
        let initial = store
            .central_audit_export_cursor(&source, &organization, &team)
            .await
            .unwrap();
        assert_eq!(initial.source_sequence, 0);
        assert_eq!(initial.local_sequence, 0);
        let next = CentralAuditExportCursor {
            source_sequence: 1,
            local_sequence: 3,
            chain_head: "a".repeat(64),
            ..initial.clone()
        };
        let event = Event {
            id: Id("event-3".into()),
            sequence: 3,
            timestamp: Utc::now(),
            scope: Scope {
                organization_id: organization.clone(),
                team_id: team.clone(),
                actor_id: Id("actor-a".into()),
                goal_id: None,
                task_id: None,
            },
            session_id: None,
            turn_id: None,
            kind: "turn.completed".into(),
            payload: serde_json::json!({"content":"not in envelope"}),
        };
        let signer =
            CentralAuditSigner::from_base64("source-key", &URL_SAFE_NO_PAD.encode([51_u8; 32]))
                .unwrap();
        let envelope = signer
            .sign(CentralAuditBatchPayload {
                schema_version: CENTRAL_AUDIT_SCHEMA_VERSION,
                batch_id: Id("batch-a".into()),
                source_id: source.clone(),
                organization_id: organization.clone(),
                team_id: team.clone(),
                previous_sequence: 0,
                previous_chain_hash: "0".repeat(64),
                generated_at: Utc::now(),
                encrypted_content: None,
                records: vec![
                    CentralAuditRecord::metadata_only(&event, source.clone(), 1, "a".repeat(64))
                        .unwrap(),
                ],
            })
            .unwrap();
        let pending = store
            .put_central_audit_pending_export(&initial, &envelope)
            .await
            .unwrap();
        assert_eq!(pending, envelope);
        assert_eq!(
            store
                .central_audit_pending_export(&source, &organization, &team)
                .await
                .unwrap(),
            Some(envelope.clone())
        );
        store
            .advance_central_audit_export_cursor(&initial, &next)
            .await
            .unwrap();
        assert_eq!(
            store
                .central_audit_export_cursor(&source, &organization, &team)
                .await
                .unwrap(),
            next
        );
        store
            .advance_central_audit_export_cursor(&initial, &next)
            .await
            .unwrap();
        store
            .delete_central_audit_pending_export(
                &source,
                &envelope.payload.batch_id,
                &envelope.payload.sha256().unwrap(),
            )
            .await
            .unwrap();
        assert!(
            store
                .central_audit_pending_export(&source, &organization, &team)
                .await
                .unwrap()
                .is_none()
        );
        let conflicting = CentralAuditExportCursor {
            source_sequence: 2,
            local_sequence: 4,
            chain_head: "b".repeat(64),
            ..initial.clone()
        };
        assert!(
            store
                .advance_central_audit_export_cursor(&initial, &conflicting)
                .await
                .is_err()
        );
        assert!(
            store
                .central_audit_export_cursor(&source, &organization, &Id("team-b".into()))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn encrypted_store_keeps_sensitive_content_out_of_sqlite() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[9_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "encryption test".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let message = store
            .append_message(
                &team,
                &session.id,
                "user",
                serde_json::json!({"text":"customer-secret-phrase"}),
            )
            .await
            .unwrap();
        let raw: String = sqlx::query_scalar("SELECT content_json FROM messages WHERE id=?")
            .bind(&message.id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:v1:key-1:"));
        assert!(!raw.contains("customer-secret-phrase"));
        assert_eq!(
            store.list_messages(&team, &session.id).await.unwrap()[0].content,
            message.content
        );

        let event = Event {
            id: Id("event-encrypted".into()),
            sequence: 1,
            timestamp: Utc::now(),
            scope: team.clone(),
            session_id: Some(session.id.clone()),
            turn_id: Some(message.turn_id.clone()),
            kind: "model.delta".into(),
            payload: serde_json::json!({
                "item_id":"item-encrypted",
                "text":"model-output-canary-4dc8388f",
            }),
        };
        store.append_event(&event, &"cd".repeat(32)).await.unwrap();
        let raw_event: String =
            sqlx::query_scalar("SELECT payload_json FROM audit_events WHERE id=?")
                .bind(&event.id.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(raw_event.starts_with("enc:v1:key-1:"));
        assert!(!raw_event.contains("model-output-canary-4dc8388f"));
        assert_eq!(
            store.list_team_events(&team, 0, 10).await.unwrap()[0].payload,
            event.payload
        );

        let wrong = SensitiveCodec::encrypted("key-1", &[8_u8; 32]).unwrap();
        assert!(
            wrong
                .open_text(&team, "messages", &message.id, "content_json", &raw)
                .is_err()
        );
    }

    #[tokio::test]
    async fn session_preferences_are_scoped_persisted_and_default_to_manual() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[6_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "preferences".into(),
                model: "model-a".into(),
            })
            .await
            .unwrap();
        let defaults = store
            .get_session_preferences(&team, &session.id)
            .await
            .unwrap();
        assert_eq!(defaults.permission_mode, PermissionMode::Manual);
        assert_eq!(defaults.assistant_alias, "Opencoding");
        assert_eq!(defaults.source, "default");
        let updated = store
            .update_session_preferences(
                &session.id,
                UpdateSessionPreferences {
                    scope: team.clone(),
                    permission_mode: Some(PermissionMode::AcceptEdits),
                    assistant_alias: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(updated.assistant_alias, "Opencoding");
        assert_eq!(updated.source, "session");
        let renamed = store
            .update_session_preferences(
                &session.id,
                UpdateSessionPreferences {
                    scope: team.clone(),
                    permission_mode: None,
                    assistant_alias: Some("橙子".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(renamed.assistant_alias, "橙子");
        let workspace = store
            .update_session_preferences(
                &session.id,
                UpdateSessionPreferences {
                    scope: team.clone(),
                    permission_mode: Some(PermissionMode::Workspace),
                    assistant_alias: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(workspace.permission_mode, PermissionMode::Workspace);
        assert!(
            store
                .update_session_preferences(
                    &session.id,
                    UpdateSessionPreferences {
                        scope: team.clone(),
                        permission_mode: None,
                        assistant_alias: Some(" ".into()),
                    },
                )
                .await
                .is_err()
        );
        assert!(
            store
                .get_session_preferences(&scope("team_b"), &session.id)
                .await
                .is_err()
        );
        let changed = store
            .update_session_model(&team, &session.id, "model-b")
            .await
            .unwrap();
        assert_eq!(changed.model, "model-b");
        let raw: String = sqlx::query_scalar("SELECT model FROM sessions WHERE id=?")
            .bind(&session.id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(!raw.contains("model-b"));
    }

    #[tokio::test]
    async fn team_goal_run_is_idempotent_revisioned_and_actor_scoped() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let goal = store
            .create_team_goal(CreateTeamGoal {
                scope: team.clone(),
                title: "Ship controlled automation".into(),
                outcome_definition: "Every Task reaches Review".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let input = ContinueTeamGoal {
            scope: team.clone(),
            workspace_uri: "file:///repo".into(),
            model: "model-a".into(),
            idempotency_key: "run-key".into(),
            max_attempts: 2,
            max_runtime_seconds: 600,
            max_cost_micros: 1_000_000,
        };
        let (created, inserted) = store
            .get_or_create_team_goal_run(&goal.id, &input)
            .await
            .unwrap();
        assert!(inserted);
        assert_eq!(created.status, TeamGoalRunStatus::Active);
        let (replayed, inserted) = store
            .get_or_create_team_goal_run(&goal.id, &input)
            .await
            .unwrap();
        assert!(!inserted);
        assert_eq!(replayed.id, created.id);
        assert!(
            store
                .update_team_goal_run_status(
                    &team,
                    &created.id,
                    TeamGoalRunStatus::Paused,
                    created.revision + 1,
                )
                .await
                .is_err()
        );
        let paused = store
            .update_team_goal_run_status(
                &team,
                &created.id,
                TeamGoalRunStatus::Paused,
                created.revision,
            )
            .await
            .unwrap();
        assert_eq!(paused.status, TeamGoalRunStatus::Paused);
        assert!(paused.revision > created.revision);
        let other_actor = Scope {
            actor_id: Id("other".into()),
            ..team.clone()
        };
        assert!(
            store
                .get_team_goal_run(&other_actor, &created.id)
                .await
                .is_err()
        );
        let cancelled = store
            .update_team_goal_run_status(
                &team,
                &created.id,
                TeamGoalRunStatus::Cancelled,
                paused.revision,
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status, TeamGoalRunStatus::Cancelled);
        assert!(
            store
                .update_team_goal_run_status(
                    &team,
                    &created.id,
                    TeamGoalRunStatus::Active,
                    cancelled.revision,
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn session_goal_is_encrypted_revisioned_scoped_and_budgeted() {
        let store = Store::connect_encrypted("sqlite::memory:", "goal-key", &[4_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "goal".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let goal = store
            .set_session_goal(
                &session.id,
                SetSessionGoal {
                    scope: team.clone(),
                    objective: "Finish customer-secret-migration and pass tests".into(),
                    auto_continue: true,
                    token_budget: Some(100),
                },
            )
            .await
            .unwrap();
        assert_eq!(goal.status, SessionGoalStatus::Active);
        assert_eq!(goal.revision, 1);
        let raw: String = sqlx::query_scalar("SELECT objective FROM session_goals WHERE id=?")
            .bind(&goal.id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:v1:goal-key:"));
        assert!(!raw.contains("customer-secret-migration"));
        assert!(
            store
                .get_session_goal(&scope("team_b"), &session.id)
                .await
                .is_err()
        );

        let paused = store
            .update_session_goal(
                &session.id,
                UpdateSessionGoal {
                    scope: team.clone(),
                    objective: None,
                    status: Some(SessionGoalStatus::Paused),
                    auto_continue: None,
                    expected_revision: goal.revision,
                    blocked_reason: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(paused.status, SessionGoalStatus::Paused);
        assert!(matches!(
            store
                .update_session_goal(
                    &session.id,
                    UpdateSessionGoal {
                        scope: team.clone(),
                        objective: None,
                        status: Some(SessionGoalStatus::Active),
                        auto_continue: None,
                        expected_revision: goal.revision,
                        blocked_reason: None,
                    },
                )
                .await,
            Err(StorageError::InvalidState(_))
        ));
        let resumed = store
            .update_session_goal(
                &session.id,
                UpdateSessionGoal {
                    scope: team.clone(),
                    objective: None,
                    status: Some(SessionGoalStatus::Active),
                    auto_continue: None,
                    expected_revision: paused.revision,
                    blocked_reason: None,
                },
            )
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let exhausted = store
            .record_session_goal_turn(&team, &session.id, &turn.id, 60, 40)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exhausted.status, SessionGoalStatus::Blocked);
        assert_eq!(exhausted.continuation_count, 1);
        assert_eq!(exhausted.input_tokens + exhausted.output_tokens, 100);
        assert_eq!(
            exhausted.blocked_reason.as_deref(),
            Some("The configured token budget has been exhausted.")
        );
        let cleared = store
            .clear_session_goal(&team, &session.id, exhausted.revision)
            .await
            .unwrap();
        assert_eq!(cleared.id, resumed.id);
        assert!(
            store
                .get_session_goal(&team, &session.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn question_request_is_encrypted_scoped_and_answered_only_once() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[7_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "question".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let request = QuestionRequest {
            id: Id("question-one".into()),
            session_id: session.id.clone(),
            turn_id: turn.id,
            item_id: Id("question-item-one".into()),
            questions: vec![opencoding_protocol::QuestionPrompt {
                id: "approach".into(),
                header: "Approach".into(),
                question: "Choose customer-secret-option?".into(),
                options: vec![
                    opencoding_protocol::QuestionOption {
                        label: "Safe".into(),
                        description: "Use the bounded path.".into(),
                    },
                    opencoding_protocol::QuestionOption {
                        label: "Fast".into(),
                        description: "Use the smaller path.".into(),
                    },
                ],
            }],
            allow_other: true,
            requested_by: team.actor_id.clone(),
            requested_at: Utc::now(),
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
            status: QuestionStatus::Pending,
            answers: Vec::new(),
            answered_by: None,
            answered_at: None,
            revision: 1,
        };
        store
            .create_question_request(&team, &request)
            .await
            .unwrap();
        let raw: String =
            sqlx::query_scalar("SELECT questions_json FROM question_requests WHERE id=?")
                .bind(&request.id.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(raw.starts_with("enc:v1:key-1:"));
        assert!(!raw.contains("customer-secret-option"));
        assert!(
            store
                .get_question_request(&scope("team_b"), &request.id)
                .await
                .is_err()
        );
        let due = store.list_due_question_requests(Utc::now()).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, team);
        assert_eq!(due[0].1.id, request.id);
        let answers = vec![QuestionAnswer {
            question_id: "approach".into(),
            answer: "Safe".into(),
        }];
        let (first, second) = tokio::join!(
            store.resolve_question_request(&team, &request.id, &answers),
            store.resolve_question_request(&team, &request.id, &answers)
        );
        assert_ne!(first.is_ok(), second.is_ok());
        let answered = store
            .get_question_request(&team, &request.id)
            .await
            .unwrap();
        assert_eq!(answered.status, QuestionStatus::Answered);
        assert_eq!(answered.answers, answers);
        assert_eq!(answered.revision, 2);
    }

    #[tokio::test]
    async fn turn_inputs_are_encrypted_idempotent_arbitrated_and_consumed_in_order() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[5_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "turn inputs".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let target = store.create_turn(&team, &session.id).await.unwrap();
        let queue_request = CreateTurnInput {
            scope: team.clone(),
            target_turn_id: target.id.clone(),
            mode: TurnInputMode::Queue,
            content: serde_json::json!("customer-secret-queued-input"),
            idempotency_key: "queue-one".into(),
        };
        let queued = store
            .create_turn_input(&session.id, queue_request.clone())
            .await
            .unwrap();
        let replay = store
            .create_turn_input(&session.id, queue_request)
            .await
            .unwrap();
        assert_eq!(replay.id, queued.id);
        let raw: String = sqlx::query_scalar("SELECT content_json FROM turn_inputs WHERE id=?")
            .bind(&queued.id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:v1:key-1:"));
        assert!(!raw.contains("customer-secret-queued-input"));
        assert!(
            store
                .get_turn_input(&scope("team_b"), &queued.id)
                .await
                .is_err()
        );

        let steer = |key: &str| CreateTurnInput {
            scope: team.clone(),
            target_turn_id: target.id.clone(),
            mode: TurnInputMode::Steer,
            content: serde_json::json!(format!("steer {key}")),
            idempotency_key: key.into(),
        };
        let (left, right) = tokio::join!(
            store.create_turn_input(&session.id, steer("steer-left")),
            store.create_turn_input(&session.id, steer("steer-right"))
        );
        assert_ne!(left.is_ok(), right.is_ok());
        let winning_steer = left.or(right).unwrap();

        store
            .update_turn(&team, &target.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();
        let claimed = store.claim_next_ready_turn_input().await.unwrap().unwrap();
        assert_eq!(claimed.id, winning_steer.id);
        assert_eq!(claimed.status, TurnInputStatus::Processing);
        let (consumed, successor, message) = store
            .consume_turn_input_into_turn(&claimed.id)
            .await
            .unwrap();
        assert_eq!(consumed.status, TurnInputStatus::Consumed);
        assert_eq!(consumed.resulting_turn_id.as_ref(), Some(&successor.id));
        assert_eq!(message.content, winning_steer.content);
        assert!(store.claim_next_ready_turn_input().await.unwrap().is_none());

        store
            .update_turn(&team, &successor.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();
        let claimed_queue = store.claim_next_ready_turn_input().await.unwrap().unwrap();
        assert_eq!(claimed_queue.id, queued.id);
        let (consumed_queue, _, queue_message) = store
            .consume_turn_input_into_turn(&claimed_queue.id)
            .await
            .unwrap();
        assert_eq!(consumed_queue.status, TurnInputStatus::Consumed);
        assert_eq!(queue_message.content, queued.content);
        assert!(
            store
                .list_pending_turn_inputs(&team, &session.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn artifact_content_is_encrypted_and_team_scoped() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[6_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "artifact".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let artifact = Artifact {
            metadata: opencoding_protocol::ArtifactMetadata {
                id: Id("artifact-one".into()),
                session_id: session.id.clone(),
                turn_id: turn.id,
                item_id: Id("artifact-item-one".into()),
                title: "Customer report".into(),
                media_type: "text/markdown".into(),
                byte_length: Some(22),
                created_at: Utc::now(),
            },
            content: serde_json::json!("# customer-secret-report"),
        };
        store.create_artifact(&team, &artifact).await.unwrap();
        let bob = Scope {
            actor_id: Id("usr_bob".into()),
            ..team.clone()
        };
        let bob_session = store
            .create_session(CreateSession {
                scope: bob.clone(),
                workspace_uri: "file:///repo".into(),
                title: "bob artifact".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let bob_turn = store.create_turn(&bob, &bob_session.id).await.unwrap();
        let bob_artifact = Artifact {
            metadata: opencoding_protocol::ArtifactMetadata {
                id: Id("artifact-bob".into()),
                session_id: bob_session.id,
                turn_id: bob_turn.id,
                item_id: Id("artifact-item-bob".into()),
                title: "Bob private report".into(),
                media_type: "text/markdown".into(),
                byte_length: Some(20),
                created_at: Utc::now() + chrono::Duration::seconds(1),
            },
            content: serde_json::json!("# bob-private-report"),
        };
        store.create_artifact(&bob, &bob_artifact).await.unwrap();
        let raw: (String, String) =
            sqlx::query_as("SELECT title,content_json FROM artifacts WHERE id=?")
                .bind(&artifact.metadata.id.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(raw.0.starts_with("enc:v1:key-1:"));
        assert!(raw.1.starts_with("enc:v1:key-1:"));
        assert!(!raw.1.contains("customer-secret-report"));
        assert_eq!(
            store
                .get_artifact(&team, &artifact.metadata.id)
                .await
                .unwrap(),
            artifact
        );
        assert_eq!(
            store
                .list_artifacts(&team, None, None, None, 10)
                .await
                .unwrap(),
            vec![artifact.clone()]
        );
        assert_eq!(
            store
                .list_artifacts(&bob, None, None, None, 1)
                .await
                .unwrap(),
            vec![bob_artifact]
        );
        assert!(
            store
                .list_artifacts(&scope("team_b"), None, None, None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .get_artifact(&scope("team_b"), &artifact.metadata.id)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn attachments_are_encrypted_scoped_and_bound_atomically_to_a_turn() {
        let store = Store::connect_encrypted("sqlite::memory:", "key-1", &[7_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "attachments".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let content = b"customer-secret-attachment";
        let attachment = store
            .create_attachment(
                &session.id,
                CreateAttachment {
                    scope: team.clone(),
                    file_name: "notes.txt".into(),
                    media_type: "text/plain".into(),
                    content_base64: STANDARD.encode(content),
                },
            )
            .await
            .unwrap();
        let raw: (String, String, String) =
            sqlx::query_as("SELECT file_name,content_base64,sha256 FROM attachments WHERE id=?")
                .bind(&attachment.metadata.id.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(raw.0.starts_with("enc:v1:key-1:"));
        assert!(raw.1.starts_with("enc:v1:key-1:"));
        assert!(raw.2.starts_with("enc:v1:key-1:"));
        assert!(!raw.1.contains("customer-secret"));
        let (turn, _) = store
            .create_turn_with_user_message_and_attachments_if_idle(
                &team,
                &session.id,
                serde_json::json!("inspect the attachment"),
                std::slice::from_ref(&attachment.metadata.id),
            )
            .await
            .unwrap();
        let attached = store.list_turn_attachments(&team, &turn.id).await.unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].content_base64, STANDARD.encode(content));
        assert_eq!(attached[0].metadata.turn_id.as_ref(), Some(&turn.id));
        assert!(
            store
                .delete_draft_attachment(&team, &attachment.metadata.id)
                .await
                .is_err()
        );
        assert!(
            store
                .get_attachment(&scope("team_b"), &attachment.metadata.id)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn side_conversations_are_actor_private_forks_with_terminal_lifecycle() {
        let store = Store::in_memory().await.unwrap();
        let team = scope("team_a");
        let source = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo".into(),
                title: "source".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let fork = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: source.workspace_uri.clone(),
                title: "side".into(),
                model: source.model.clone(),
            })
            .await
            .unwrap();
        store
            .record_session_fork(&team, &fork.id, &source.id, None)
            .await
            .unwrap();
        let side = store
            .create_side_conversation(&team, &source.id, &fork.id, None)
            .await
            .unwrap();
        assert_eq!(side.status, SideConversationStatus::Active);
        assert_eq!(
            store.list_side_conversations(&team).await.unwrap(),
            vec![side.clone()]
        );

        let mut other_actor = team.clone();
        other_actor.actor_id = Id("other-user".into());
        assert!(
            store
                .list_side_conversations(&other_actor)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .get_side_conversation(&other_actor, &side.id)
                .await
                .is_err()
        );

        let promoted = store
            .update_side_conversation_status(&team, &side.id, SideConversationStatus::Promoted)
            .await
            .unwrap();
        assert_eq!(promoted.status, SideConversationStatus::Promoted);
        assert!(
            store
                .update_side_conversation_status(&team, &side.id, SideConversationStatus::Closed,)
                .await
                .is_err()
        );

        let competing_fork = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: source.workspace_uri.clone(),
                title: "competing-side".into(),
                model: source.model.clone(),
            })
            .await
            .unwrap();
        store
            .record_session_fork(&team, &competing_fork.id, &source.id, None)
            .await
            .unwrap();
        let competing_side = store
            .create_side_conversation(&team, &source.id, &competing_fork.id, None)
            .await
            .unwrap();
        let promote_store = store.clone();
        let promote_team = team.clone();
        let promote_id = competing_side.id.clone();
        let close_store = store.clone();
        let close_team = team.clone();
        let close_id = competing_side.id.clone();
        let (promoted, closed) = tokio::join!(
            async move {
                promote_store
                    .update_side_conversation_status(
                        &promote_team,
                        &promote_id,
                        SideConversationStatus::Promoted,
                    )
                    .await
            },
            async move {
                close_store
                    .update_side_conversation_status(
                        &close_team,
                        &close_id,
                        SideConversationStatus::Closed,
                    )
                    .await
            },
        );
        assert_eq!(
            usize::from(promoted.is_ok()) + usize::from(closed.is_ok()),
            1,
            "exactly one competing terminal transition must win"
        );
        assert!(matches!(
            store
                .get_side_conversation(&team, &competing_side.id)
                .await
                .unwrap()
                .status,
            SideConversationStatus::Promoted | SideConversationStatus::Closed
        ));
    }

    #[tokio::test]
    async fn mcp_installations_are_encrypted_team_scoped_idempotent_and_confirmed_on_remove() {
        let store = Store::connect_encrypted("sqlite::memory:", "mcp-key", &[3_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let server = McpServerSpec {
            id: "local-tools".into(),
            program: "/usr/bin/example-mcp".into(),
            args: vec!["--stdio".into()],
            environment_handles: Default::default(),
            timeout_ms: 30_000,
        };
        let installed = store
            .install_mcp_server(&team, &server, "permission-digest")
            .await
            .unwrap();
        assert_eq!(installed.server, server);
        assert_eq!(installed.revision, 1);
        assert_eq!(
            store
                .install_mcp_server(&team, &installed.server, "permission-digest")
                .await
                .unwrap(),
            installed
        );
        assert_eq!(
            store.list_mcp_installations(&team).await.unwrap(),
            vec![installed.clone()]
        );
        assert!(
            store
                .list_mcp_installations(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .remove_mcp_server(&team, "local-tools", "stale-digest")
                .await
                .is_err()
        );
        assert_eq!(
            store
                .remove_mcp_server(&team, "local-tools", "permission-digest")
                .await
                .unwrap(),
            installed
        );
        assert!(
            store
                .list_mcp_installations(&team)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn mcp_http_installations_encrypt_endpoint_and_secret_handles_and_are_team_scoped() {
        let store = Store::connect_encrypted("sqlite::memory:", "mcp-http-key", &[4_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let server = McpHttpServerSpec {
            id: "remote-tools".into(),
            endpoint: "https://mcp.example.test/private-path".into(),
            header_handles: std::collections::BTreeMap::from([(
                "Authorization".into(),
                "REMOTE_MCP_ACCESS_TOKEN".into(),
            )]),
            oauth: None,
            timeout_ms: 30_000,
        };
        let installed = store
            .install_mcp_http_server(&team, &server, "permission-digest")
            .await
            .unwrap();
        assert_eq!(installed.server, server);
        let ciphertext: String =
            sqlx::query_scalar("SELECT spec_json FROM mcp_http_installations WHERE server_id=?")
                .bind("remote-tools")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!ciphertext.contains("private-path"));
        assert!(!ciphertext.contains("REMOTE_MCP_ACCESS_TOKEN"));
        assert_eq!(
            store.list_mcp_http_installations(&team).await.unwrap(),
            vec![installed.clone()]
        );
        assert!(
            store
                .list_mcp_http_installations(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .remove_mcp_http_server(&team, "remote-tools", "stale")
                .await
                .is_err()
        );
        store
            .remove_mcp_http_server(&team, "remote-tools", "permission-digest")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn marketplace_upgrade_persists_a_rebound_source_with_the_same_manifest_digest() {
        let store = Store::connect_encrypted("sqlite::memory:", "marketplace-key", &[9_u8; 32])
            .await
            .unwrap();
        let team = scope("team_marketplace");
        let selected = MarketplaceSource {
            name: "local".into(),
            source_uri: "file:///selected/parent-link/source".into(),
            source_kind: opencoding_protocol::MarketplaceSourceKind::Local,
        };
        let installed = store
            .add_marketplace(&team, &selected, "same-manifest-digest")
            .await
            .unwrap();
        assert_eq!(installed.revision, 1);

        let rebound = MarketplaceSource {
            source_uri: "file:///actual/source".into(),
            ..selected.clone()
        };
        let upgraded = store
            .upgrade_marketplace(&team, "local", &rebound, "same-manifest-digest")
            .await
            .unwrap();
        assert_eq!(upgraded.source, rebound);
        assert_eq!(upgraded.revision, 2);

        let unchanged = store
            .upgrade_marketplace(&team, "local", &rebound, "same-manifest-digest")
            .await
            .unwrap();
        assert_eq!(unchanged.revision, 2);
        let ciphertext: String = sqlx::query_scalar(
            "SELECT source_json FROM marketplace_installations WHERE marketplace_name=?",
        )
        .bind("local")
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(!ciphertext.contains("actual/source"));
    }

    #[tokio::test]
    async fn skill_instructions_are_encrypted_team_scoped_revisioned_and_digest_bound() {
        let store = Store::connect_encrypted("sqlite::memory:", "skill-key", &[2_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let instructions = "# Review\nNever expose the private validation rule.";
        let content_sha256 = format!("{:x}", Sha256::digest(instructions.as_bytes()));
        let skill = SkillSpec {
            id: "secure-review".into(),
            name: "Secure review".into(),
            description: "Review code against the local policy.".into(),
            source_uri: "file:///opt/opencoding/secure-review/SKILL.md".into(),
            activation_terms: vec!["security review".into()],
            mcp_dependencies: vec![],
            auto_match: true,
        };
        let installed = store
            .install_skill(
                &team,
                &skill,
                instructions,
                &content_sha256,
                "permission-digest",
            )
            .await
            .unwrap();
        assert!(installed.enabled);
        assert_eq!(installed.revision, 1);
        let ciphertext: String =
            sqlx::query_scalar("SELECT instructions FROM skill_installations WHERE skill_id=?")
                .bind("secure-review")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!ciphertext.contains("private validation rule"));
        let contexts = store.list_enabled_skill_contexts(&team).await.unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].instructions, instructions);
        assert_eq!(contexts[0].installation, installed);
        assert!(
            store
                .list_skill_installations(&scope("team_b"))
                .await
                .unwrap()
                .is_empty()
        );

        let disabled = store
            .set_skill_enabled(&team, "secure-review", false, 1)
            .await
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.revision, 2);
        assert!(
            store
                .set_skill_enabled(&team, "secure-review", true, 1)
                .await
                .is_err()
        );
        assert!(
            store
                .list_enabled_skill_contexts(&team)
                .await
                .unwrap()
                .is_empty()
        );
        let enabled = store
            .set_skill_enabled(&team, "secure-review", true, 2)
            .await
            .unwrap();
        assert!(enabled.enabled);
        assert_eq!(enabled.revision, 3);
        assert!(
            store
                .remove_skill(&team, "secure-review", "stale")
                .await
                .is_err()
        );
        store
            .remove_skill(&team, "secure-review", "permission-digest")
            .await
            .unwrap();
        assert!(
            store
                .list_skill_installations(&team)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn background_terminal_state_and_output_are_encrypted_actor_scoped_and_revisioned() {
        let store = Store::connect_encrypted("sqlite::memory:", "terminal-key", &[8_u8; 32])
            .await
            .unwrap();
        let team = scope("team_a");
        let session = store
            .create_session(CreateSession {
                scope: team.clone(),
                workspace_uri: "file:///repo/".into(),
                title: "terminal".into(),
                model: "model".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&team, &session.id).await.unwrap();
        let spec = BackgroundTerminalSpec {
            session_id: session.id,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf terminal-secret".into()],
            environment_handles: Default::default(),
            working_directory_uri: "file:///repo/".into(),
            rows: 24,
            cols: 80,
            max_runtime_seconds: 60,
        };
        let created = store
            .create_background_terminal(&team, &spec, &"a".repeat(64), &turn.id)
            .await
            .unwrap();
        assert_eq!(created.status, BackgroundTerminalStatus::Starting);
        let raw: (String, String, String) = sqlx::query_as(
            "SELECT spec_json,program,working_directory_uri FROM background_terminals WHERE id=?",
        )
        .bind(&created.id.0)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(raw.0.starts_with("enc:v1:terminal-key:"));
        assert!(!raw.0.contains("terminal-secret"));
        assert!(!raw.1.contains("/bin/sh"));
        assert!(!raw.2.contains("file:///repo/"));

        let running = store
            .set_background_terminal_running(&team, &created.id)
            .await
            .unwrap();
        assert_eq!(running.status, BackgroundTerminalStatus::Running);
        store
            .append_background_terminal_output(&team, &created.id, b"terminal-secret")
            .await
            .unwrap();
        let raw_output: String =
            sqlx::query_scalar("SELECT output_base64 FROM background_terminals WHERE id=?")
                .bind(&created.id.0)
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert!(!raw_output.contains("terminal-secret"));
        assert_eq!(
            store
                .get_background_terminal_output(&team, &created.id)
                .await
                .unwrap(),
            b"terminal-secret"
        );
        let oversized = vec![b'x'; 1024 * 1024 + 17];
        let bounded = store
            .append_background_terminal_output(&team, &created.id, &oversized)
            .await
            .unwrap();
        assert_eq!(
            bounded.output_byte_length,
            b"terminal-secret".len() as u64 + oversized.len() as u64
        );
        assert!(bounded.output_truncated);
        let bounded_output = store
            .get_background_terminal_output(&team, &created.id)
            .await
            .unwrap();
        assert_eq!(bounded_output.len(), 1024 * 1024);
        assert!(bounded_output.starts_with(b"terminal-secret"));
        let resized = store
            .resize_background_terminal(&team, &created.id, 40, 120)
            .await
            .unwrap();
        assert_eq!((resized.rows, resized.cols), (40, 120));
        let finished = store
            .finish_background_terminal(
                &team,
                &created.id,
                BackgroundTerminalStatus::Exited,
                Some(0),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(finished.status, BackgroundTerminalStatus::Exited);
        assert_eq!(finished.exit_code, Some(0));
        assert!(finished.revision > created.revision);

        let mut other_actor = team.clone();
        other_actor.actor_id = Id("other".into());
        assert!(
            store
                .get_background_terminal(&other_actor, &created.id)
                .await
                .is_err()
        );

        let restart_terminal = store
            .create_background_terminal(&team, &spec, &"b".repeat(64), &turn.id)
            .await
            .unwrap();
        store
            .set_background_terminal_running(&team, &restart_terminal.id)
            .await
            .unwrap();
        assert_eq!(store.mark_background_terminals_orphaned().await.unwrap(), 1);
        assert_eq!(
            store
                .get_background_terminal(&team, &restart_terminal.id)
                .await
                .unwrap()
                .status,
            BackgroundTerminalStatus::Orphaned
        );
    }
}
