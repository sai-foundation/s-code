//! SQLite persistence for the remote registry. Every status transition is
//! decided by the shared domain gate inside the same `BEGIN IMMEDIATE`
//! transaction that records the receipt, exactly like the local shop, so
//! concurrent receipts cannot verify twice, lose a receipt, count one
//! principal twice, resurrect a deprecated skill or split the receipt from
//! its transition.
use crate::auth::{generate_session_id, generate_token, token_hash};
use chrono::{DateTime, Utc};
use s_code_skill_shop::{
    ChallengeEvidence, ChallengeItem, ChallengeKind, ChallengeStatus, ChallengeSubmission,
    ComparisonItem, ComparisonSubmission, ComparisonSummary, ForkSubmission, Lineage, LineageNode,
    MAX_CHALLENGES_PER_PRINCIPAL, MAX_CHALLENGES_PER_SKILL, MAX_COMPARISONS_PER_PRINCIPAL,
    MAX_FORKS_PER_SKILL, MAX_FORKS_PER_TEAM_PER_LINEAGE, MAX_FORKS_PER_TEAM_PER_SKILL,
    MAX_LINEAGE_DEPTH, MAX_LINEAGE_NODES, MAX_SKILL_REASON_CHARS, PoisoningVerdict, ReceiptSummary,
    SAFETY_DEPRECATION_REASON, SKILL_GATE_VERSION, SUPERSESSION_GATE_VERSION, SharedScope,
    SkillAggregate, SkillArtifact, SkillPrincipal, SkillProvenance, SkillPublication,
    SkillReceiptItem, SkillReceiptSubmission, SkillSafety, SkillStatus, SkillTransition,
    SkillVisibility, SupersessionTransition, active_successor, aggregate_receipts,
    challenge_digest, comparison_verdict, receipt_verdict, skill_gate, supersession_gate,
    validate_challenge, validate_comparison, validate_fork, validate_publication, validate_receipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, str::FromStr, time::Duration};

/// Receipts joined with their evaluator's current standing and the skill's
/// scope, so authority is evaluated against the present facts as well as
/// the recorded ones: a disabled principal's receipts stop counting, a
/// revoked capability stops counting, and re-granting restores, without any
/// receipt being rewritten. The joins are outer joins, so a receipt whose
/// evaluator or skill row is gone is unreadable rather than silently absent.
const RECEIPTS_OLDEST_FIRST: &str = "SELECT r.*, p.disabled AS evaluator_disabled, p.authorized_evaluator AS evaluator_authorized, (p.organization_id = s.organization_id AND p.team_id = s.team_id) AS evaluator_in_team FROM receipts r LEFT JOIN principals p ON p.id = r.principal_id LEFT JOIN skills s ON s.id = r.skill_id WHERE r.skill_id=? ORDER BY r.created_at ASC, r.id ASC";
const RECEIPTS_NEWEST_FIRST: &str = "SELECT r.*, p.disabled AS evaluator_disabled, p.authorized_evaluator AS evaluator_authorized, (p.organization_id = s.organization_id AND p.team_id = s.team_id) AS evaluator_in_team FROM receipts r LEFT JOIN principals p ON p.id = r.principal_id LEFT JOIN skills s ON s.id = r.skill_id WHERE r.skill_id=? ORDER BY r.created_at DESC, r.id DESC";

/// How long a web session stays valid after login.
pub const WEB_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

const RECEIPT_BY_ID: &str = "SELECT r.*, p.disabled AS evaluator_disabled, p.authorized_evaluator AS evaluator_authorized, (p.organization_id = s.organization_id AND p.team_id = s.team_id) AS evaluator_in_team FROM receipts r LEFT JOIN principals p ON p.id = r.principal_id LEFT JOIN skills s ON s.id = r.skill_id WHERE r.id=?";

/// Comparisons joined with the evaluator's current standing, team and
/// membership of the PARENT's team. Link authority is decided against the
/// lineage (see `lineage_team`) and parent authority against the parent,
/// with the same live rule receipts use. Outer joins, as for receipts.
const COMPARISONS_SELECT: &str = "SELECT c.*, p.disabled AS evaluator_disabled, p.authorized_evaluator AS evaluator_authorized, p.organization_id AS evaluator_organization, p.team_id AS evaluator_team, (p.organization_id = s.organization_id AND p.team_id = s.team_id) AS evaluator_in_parent_team FROM comparisons c LEFT JOIN principals p ON p.id = c.principal_id LEFT JOIN skills s ON s.id = c.parent_id WHERE c.fork_id=?";

/// The revision of the rules the store applies to installed links beyond
/// the domain gate (authority, persistent vetoes, unreadable evidence).
/// Bump it with any change to how an installed link is judged; it stays
/// below 1000 so it never collides with the next gate version.
const LINK_RULES_REVISION: i64 = 3;
const _: () = assert!(LINK_RULES_REVISION < 1000);

/// The generation of the rules installed links are judged by: a new gate
/// version or a new link-rules revision. A database whose `user_version` is
/// older has every link re-checked once at start.
const REVALIDATION_EPOCH: i64 = SUPERSESSION_GATE_VERSION as i64 * 1000 + LINK_RULES_REVISION;

pub const MAX_LIST_LIMIT: u32 = 100;
pub const DEFAULT_LIST_LIMIT: u32 = 50;

/// A bounded page size for list endpoints.
fn list_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT)
}

/// One page of a forum list, newest first: at most `limit` items (1 to
/// 100, default 50), strictly older than the item named by `before` when
/// given, so a client reads a whole list by passing the last id it saw.
#[derive(Clone, Debug, Default)]
pub struct ForumPage {
    pub limit: Option<u32>,
    pub before: Option<String>,
}

/// Which challenges a page holds; the filter is applied before the page is
/// cut, so a filtered page is never emptied by unmatched rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChallengeFilter {
    #[default]
    All,
    Open,
    EvidenceBacked,
}
const MAX_RESULT_BYTES: usize = 64 * 1024;
/// The deprecation reason for a skill whose arm failed its safety probe in an
/// authoritative comparison.
pub const COMPARISON_SAFETY_REASON: &str = "comparison_safety_failed";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("storage failure: {0}")]
    Storage(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

/// A server-side identity. Tokens never appear here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub display_name: String,
    pub organization_id: String,
    pub team_id: String,
    pub disabled: bool,
    /// Granted by the registry administrator only: this principal's
    /// receipts may verify or deprecate public skills of other teams. Never
    /// taken from a request.
    #[serde(default)]
    pub authorized_evaluator: bool,
    pub created_at: DateTime<Utc>,
}

impl Principal {
    pub fn scope(&self) -> SharedScope {
        SharedScope {
            organization_id: self.organization_id.clone(),
            team_id: self.team_id.clone(),
        }
    }

    pub fn as_skill_principal(&self) -> SkillPrincipal {
        SkillPrincipal {
            id: self.id.clone(),
            display_name: Some(self.display_name.clone()),
        }
    }
}

/// Who is looking: an authenticated principal or nobody.
#[derive(Clone, Debug)]
pub struct Viewer {
    pub principal: Option<Principal>,
}

impl Viewer {
    pub fn anonymous() -> Self {
        Self { principal: None }
    }

    fn in_team(&self, organization_id: &str, team_id: &str) -> bool {
        self.principal.as_ref().is_some_and(|principal| {
            principal.organization_id == organization_id && principal.team_id == team_id
        })
    }

    /// The visibility rules: a team member sees the team's skills in every
    /// status; an authorized evaluator additionally sees public candidates,
    /// because evaluating them is its job; everyone else sees only public
    /// skills that are verified or deprecated. Candidates are never visible
    /// to anonymous readers or to ordinary principals of other teams.
    pub fn can_see(&self, skill: &SkillArtifact) -> bool {
        if self.in_team(
            &skill.shared_scope.organization_id,
            &skill.shared_scope.team_id,
        ) {
            return true;
        }
        if skill.visibility != SkillVisibility::Public {
            return false;
        }
        skill.status != SkillStatus::Candidate
            || self
                .principal
                .as_ref()
                .is_some_and(|principal| principal.authorized_evaluator)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub status: Option<SkillStatus>,
    pub q: Option<String>,
    pub task_family: Option<String>,
    pub model: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct RecordedReceipt {
    pub receipt: SkillReceiptItem,
    pub skill: SkillArtifact,
    pub transition: SkillTransition,
}

#[derive(Clone)]
pub struct RegistryStore {
    pool: SqlitePool,
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn new_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 12];
    let _ = getrandom::fill(&mut bytes);
    format!(
        "{prefix}_{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn row_to_principal(row: &sqlx::sqlite::SqliteRow) -> Result<Principal, StoreError> {
    let disabled: i64 = row.try_get("disabled")?;
    let authorized_evaluator: i64 = row.try_get("authorized_evaluator")?;
    Ok(Principal {
        id: row.try_get("id")?,
        display_name: row.try_get("display_name")?,
        organization_id: row.try_get("organization_id")?,
        team_id: row.try_get("team_id")?,
        disabled: disabled != 0,
        authorized_evaluator: authorized_evaluator != 0,
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_skill(row: &sqlx::sqlite::SqliteRow) -> Result<SkillArtifact, StoreError> {
    let status: String = row.try_get("status")?;
    let visibility: String = row.try_get("visibility")?;
    let sanitization_version: i64 = row.try_get("sanitization_version")?;
    let version: i64 = row.try_get("version")?;
    let provenance: String = row.try_get("provenance_json")?;
    Ok(SkillArtifact {
        id: row.try_get("id")?,
        status: SkillStatus::parse(&status)
            .ok_or_else(|| StoreError::Storage(format!("unknown status {status:?}")))?,
        visibility: SkillVisibility::parse(&visibility)
            .ok_or_else(|| StoreError::Storage(format!("unknown visibility {visibility:?}")))?,
        shared_scope: SharedScope {
            organization_id: row.try_get("organization_id")?,
            team_id: row.try_get("team_id")?,
        },
        publisher: SkillPrincipal {
            id: row.try_get("publisher_id")?,
            display_name: Some(row.try_get("publisher_display")?),
        },
        lesson: row.try_get("lesson")?,
        applicability: row.try_get("applicability")?,
        content_digest: row.try_get("content_digest")?,
        sanitization_version: u32::try_from(sanitization_version).unwrap_or_default(),
        version: u32::try_from(version).unwrap_or(1),
        parent_skill_id: row.try_get("parent_skill_id")?,
        deprecation_reason: row.try_get("deprecation_reason")?,
        provenance: serde_json::from_str::<SkillProvenance>(&provenance).unwrap_or_default(),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        verified_at: row.try_get("verified_at")?,
        deprecated_at: row.try_get("deprecated_at")?,
        forked_by: row
            .try_get::<Option<String>, _>("forked_by_id")?
            .map(|id| SkillPrincipal {
                id,
                display_name: row.try_get("forked_by_display").ok().flatten(),
            }),
        responding_to_challenge_id: row.try_get("responding_to_challenge_id")?,
        superseded_by: row.try_get("superseded_by")?,
        superseded_at: row.try_get("superseded_at")?,
        summary: None,
    })
}

fn row_to_receipt(row: &sqlx::sqlite::SqliteRow) -> Result<SkillReceiptItem, StoreError> {
    let evaluator: String = row.try_get("evaluator_json")?;
    let verdict: String = row.try_get("verdict_json")?;
    let safety: String = row.try_get("safety")?;
    let independent: i64 = row.try_get("independent")?;
    let complete: i64 = row.try_get("complete")?;
    let protocol_version: i64 = row.try_get("protocol_version")?;
    // A receipt counts when it counted at submission (the recorded flag) and
    // its evaluator still qualifies now: enabled, and a member of the
    // skill's team or currently holding the authorized-evaluator capability.
    // Revoking or disabling retracts, re-granting restores, and a later
    // grant never promotes a receipt that was filed as a community receipt.
    let recorded_authority: i64 = row.try_get("authoritative")?;
    let evaluator_disabled: i64 = row.try_get("evaluator_disabled")?;
    let evaluator_authorized: i64 = row.try_get("evaluator_authorized")?;
    let evaluator_in_team: i64 = row.try_get("evaluator_in_team")?;
    Ok(SkillReceiptItem {
        id: row.try_get("id")?,
        skill_id: row.try_get("skill_id")?,
        evaluator: SkillPrincipal {
            id: row.try_get("principal_id")?,
            display_name: Some(row.try_get("principal_display")?),
        },
        evaluator_program: serde_json::from_str(&evaluator).unwrap_or(Value::Null),
        independent: independent != 0,
        authoritative: recorded_authority != 0
            && evaluator_disabled == 0
            && (evaluator_in_team != 0 || evaluator_authorized != 0),
        origin: "direct".into(),
        protocol_version: u32::try_from(protocol_version).unwrap_or_default(),
        protocol_digest: row.try_get("protocol_digest")?,
        complete: complete != 0,
        safety: SkillSafety::parse(&safety)
            .ok_or_else(|| StoreError::Storage(format!("unknown safety {safety:?}")))?,
        task_family: row.try_get("task_family")?,
        model: row.try_get("model")?,
        verdict: serde_json::from_str(&verdict).unwrap_or(Value::Null),
        created_at: row.try_get("created_at")?,
    })
}

/// A comparison with its live authorities. It counts toward the gate when
/// it counted at submission and its evaluator still holds the link: an
/// enabled authorized evaluator, or a member of `lineage_team`, the one team
/// that owns the whole lineage when there is one. It speaks for the parent
/// when it did at submission and its evaluator is still an enabled member
/// of the parent's team or an authorized evaluator.
fn row_to_comparison(
    row: &sqlx::sqlite::SqliteRow,
    lineage_team: Option<&SharedScope>,
) -> Result<ComparisonItem, StoreError> {
    let evaluator: String = row.try_get("evaluator_json")?;
    let verdict: String = row.try_get("verdict_json")?;
    let safety: String = row.try_get("fork_safety")?;
    let independent: i64 = row.try_get("independent")?;
    let complete: i64 = row.try_get("complete")?;
    let protocol_version: i64 = row.try_get("protocol_version")?;
    let recorded_authority: i64 = row.try_get("authoritative")?;
    let recorded_parent_authority: i64 = row.try_get("parent_authority")?;
    let evaluator_disabled: i64 = row.try_get("evaluator_disabled")?;
    let evaluator_authorized: i64 = row.try_get("evaluator_authorized")?;
    let evaluator_organization: String = row.try_get("evaluator_organization")?;
    let evaluator_team: String = row.try_get("evaluator_team")?;
    let evaluator_in_parent_team: i64 = row.try_get("evaluator_in_parent_team")?;
    let enabled = evaluator_disabled == 0;
    let authorized = evaluator_authorized != 0;
    let in_lineage_team = lineage_team.is_some_and(|team| {
        team.organization_id == evaluator_organization && team.team_id == evaluator_team
    });
    Ok(ComparisonItem {
        id: row.try_get("id")?,
        fork_id: row.try_get("fork_id")?,
        parent_id: row.try_get("parent_id")?,
        evaluator: SkillPrincipal {
            id: row.try_get("principal_id")?,
            display_name: Some(row.try_get("principal_display")?),
        },
        evaluator_program: serde_json::from_str(&evaluator).unwrap_or(Value::Null),
        independent: independent != 0,
        authoritative: recorded_authority != 0 && enabled && (authorized || in_lineage_team),
        parent_authority: recorded_parent_authority != 0
            && enabled
            && (authorized || evaluator_in_parent_team != 0),
        protocol_version: u32::try_from(protocol_version).unwrap_or_default(),
        protocol_digest: row.try_get("protocol_digest")?,
        complete: complete != 0,
        fork_safety: SkillSafety::parse(&safety)
            .ok_or_else(|| StoreError::Storage(format!("unknown safety {safety:?}")))?,
        task_family: row.try_get("task_family")?,
        model: row.try_get("model")?,
        verdict: serde_json::from_str(&verdict).unwrap_or(Value::Null),
        created_at: row.try_get("created_at")?,
    })
}

fn row_to_challenge(
    row: &sqlx::sqlite::SqliteRow,
    evidence: Option<ChallengeEvidence>,
) -> Result<ChallengeItem, StoreError> {
    let kind: String = row.try_get("kind")?;
    let status: String = row.try_get("status")?;
    let skill_version: i64 = row.try_get("skill_version")?;
    Ok(ChallengeItem {
        id: row.try_get("id")?,
        skill_id: row.try_get("skill_id")?,
        content_digest: row.try_get("content_digest")?,
        skill_version: u32::try_from(skill_version).unwrap_or(1),
        challenger: SkillPrincipal {
            id: row.try_get("challenger_id")?,
            display_name: Some(row.try_get("challenger_display")?),
        },
        kind: ChallengeKind::parse(&kind)
            .ok_or_else(|| StoreError::Storage(format!("unknown challenge kind {kind:?}")))?,
        claim: row.try_get("claim")?,
        applicability: row.try_get("applicability")?,
        evidence_backed: evidence.is_some(),
        evidence,
        status: ChallengeStatus::parse(&status)
            .ok_or_else(|| StoreError::Storage(format!("unknown challenge status {status:?}")))?,
        created_at: row.try_get("created_at")?,
        addressed_at: row.try_get("addressed_at")?,
        addressed_by_skill_id: row.try_get("addressed_by_skill_id")?,
    })
}

fn challenge_evidence(receipt: &SkillReceiptItem) -> ChallengeEvidence {
    ChallengeEvidence {
        receipt_id: receipt.id.clone(),
        evaluator: receipt.evaluator.clone(),
        authoritative: receipt.authoritative,
        independent: receipt.independent,
        complete: receipt.complete,
        safety: receipt.safety,
        verdict: receipt.verdict.clone(),
    }
}

impl RegistryStore {
    pub async fn open(data_dir: &Path) -> Result<Self, StoreError> {
        Self::create_private_directory(data_dir)?;
        let path = data_dir.join("registry.db");
        let store = Self::connect(&format!("sqlite://{}", path.display()), true).await?;
        Self::restrict_database_files(&path)?;
        Ok(store)
    }

    /// The data directory holds token digests, sessions and team-private
    /// text: it is created, or tightened if it already exists, to be private
    /// to the service user.
    fn create_private_directory(data_dir: &Path) -> Result<(), StoreError> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(data_dir)
            .map_err(|error| StoreError::Storage(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| StoreError::Storage(error.to_string()))?;
        }
        Ok(())
    }

    /// The database and its WAL companions are readable by the service
    /// user only. SQLite creates `-wal` and `-shm` with the database's
    /// mode, so restricting the database file first is enough for new
    /// files; existing companions are restricted too.
    fn restrict_database_files(path: &Path) -> Result<(), StoreError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["", "-wal", "-shm"] {
                let mut companion = path.as_os_str().to_owned();
                companion.push(suffix);
                let companion = std::path::PathBuf::from(companion);
                if companion.is_file() {
                    std::fs::set_permissions(&companion, std::fs::Permissions::from_mode(0o600))
                        .map_err(|error| StoreError::Storage(error.to_string()))?;
                }
            }
        }
        let _ = path;
        Ok(())
    }

    pub async fn in_memory() -> Result<Self, StoreError> {
        Self::connect("sqlite::memory:", false).await
    }

    /// Run statements on the store's own connection with foreign keys and
    /// check constraints off, as a writer behind the registry's back would.
    #[cfg(test)]
    pub(crate) async fn write_behind_the_back(&self, statements: &[String]) {
        let mut connection = self.pool.acquire().await.unwrap();
        for statement in [
            "PRAGMA foreign_keys = OFF",
            "PRAGMA ignore_check_constraints = ON",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain(statements.iter().cloned())
        .chain(
            [
                "PRAGMA ignore_check_constraints = OFF",
                "PRAGMA foreign_keys = ON",
            ]
            .map(str::to_owned),
        ) {
            sqlx::query(&statement)
                .execute(&mut *connection)
                .await
                .unwrap();
        }
    }

    async fn connect(url: &str, file_backed: bool) -> Result<Self, StoreError> {
        let mut options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        if file_backed {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }
        // An in-memory database lives exactly as long as its single
        // connection, so that connection is never recycled.
        let pool = SqlitePoolOptions::new()
            .max_connections(if file_backed { 5 } else { 1 })
            .min_connections(if file_backed { 0 } else { 1 })
            .idle_timeout(if file_backed {
                Some(Duration::from_secs(600))
            } else {
                None
            })
            .max_lifetime(if file_backed {
                Some(Duration::from_secs(1800))
            } else {
                None
            })
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|error| StoreError::Storage(error.to_string()))?;
        let store = Self { pool };
        if let Err(error) = store.revalidate_if_needed().await {
            tracing::warn!(%error, "supersession links were not re-checked; the next start tries again");
        }
        Ok(store)
    }

    // -- principals --

    /// Create a principal and return it with its token. The token is shown
    /// exactly once; only its digest is stored.
    pub async fn create_principal(
        &self,
        display_name: &str,
        organization_id: &str,
        team_id: &str,
    ) -> Result<(Principal, String), StoreError> {
        self.create_principal_with(display_name, organization_id, team_id, false)
            .await
    }

    /// Create a principal, optionally with the authorized-evaluator
    /// capability. The token is shown exactly once; only its digest is stored.
    pub async fn create_principal_with(
        &self,
        display_name: &str,
        organization_id: &str,
        team_id: &str,
        authorized_evaluator: bool,
    ) -> Result<(Principal, String), StoreError> {
        for (label, value) in [
            ("display name", display_name),
            ("organization", organization_id),
            ("team", team_id),
        ] {
            let value = value.trim();
            if value.is_empty()
                || value.chars().count() > 120
                || value.chars().any(char::is_control)
            {
                return Err(StoreError::Invalid(format!(
                    "the {label} must be 1 to 120 printable characters"
                )));
            }
        }
        let token = generate_token().map_err(StoreError::Storage)?;
        let principal = Principal {
            id: new_id("principal"),
            display_name: display_name.trim().to_owned(),
            organization_id: organization_id.trim().to_owned(),
            team_id: team_id.trim().to_owned(),
            disabled: false,
            authorized_evaluator,
            created_at: now(),
        };
        sqlx::query("INSERT INTO principals (id, display_name, organization_id, team_id, token_hash, disabled, authorized_evaluator, created_at) VALUES (?,?,?,?,?,0,?,?)")
            .bind(&principal.id).bind(&principal.display_name).bind(&principal.organization_id).bind(&principal.team_id)
            .bind(token_hash(&token)).bind(i64::from(authorized_evaluator)).bind(principal.created_at)
            .execute(&self.pool).await?;
        self.record_event("principal.created", None, Some(&principal.id), serde_json::json!({"organization_id": principal.organization_id, "team_id": principal.team_id, "authorized_evaluator": authorized_evaluator})).await?;
        Ok((principal, token))
    }

    /// Grant or revoke the authorized-evaluator capability. Authority is
    /// evaluated against the principal's current standing whenever receipts
    /// are read, so revoking stops this principal's earlier authoritative
    /// receipts from counting and re-granting restores them; receipts filed
    /// as community receipts never become authoritative later. No receipt
    /// is rewritten and no committed status changes until the gate next
    /// runs.
    pub async fn set_authorized_evaluator(
        &self,
        id: &str,
        authorized: bool,
    ) -> Result<Principal, StoreError> {
        self.change_principal(
            id,
            "UPDATE principals SET authorized_evaluator=? WHERE id=?",
            Some(authorized),
            if authorized {
                "principal.evaluator_authorized"
            } else {
                "principal.evaluator_revoked"
            },
        )
        .await
    }

    pub async fn disable_principal(&self, id: &str) -> Result<Principal, StoreError> {
        self.change_principal(
            id,
            "UPDATE principals SET disabled=1 WHERE id=?",
            None,
            "principal.disabled",
        )
        .await
    }

    /// Change a principal's standing, record it and re-check the links its
    /// comparisons helped decide, all in one write transaction.
    async fn change_principal(
        &self,
        id: &str,
        update: &str,
        flag: Option<bool>,
        event: &str,
    ) -> Result<Principal, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut query = sqlx::query(update);
        if let Some(flag) = flag {
            query = query.bind(i64::from(flag));
        }
        let updated = query.bind(id).execute(&mut *transaction).await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::NotFound);
        }
        Self::record_event_in(
            &mut transaction,
            event,
            None,
            Some(id),
            serde_json::json!({}),
        )
        .await?;
        Self::recheck_principal_links_in(&mut transaction, id, now()).await?;
        transaction.commit().await?;
        self.get_principal(id).await?.ok_or(StoreError::NotFound)
    }

    /// The principal as it stands inside a write transaction. Authority is
    /// judged on this row, not on the one read when the request was
    /// authenticated, so a revocation or a disabling that commits first also
    /// governs a request already in flight; a principal disabled meanwhile
    /// is refused.
    async fn current_principal_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
    ) -> Result<Principal, StoreError> {
        let row = sqlx::query("SELECT * FROM principals WHERE id=?")
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(StoreError::Forbidden)?;
        let principal = row_to_principal(&row)?;
        if principal.disabled {
            return Err(StoreError::Forbidden);
        }
        Ok(principal)
    }

    pub async fn get_principal(&self, id: &str) -> Result<Option<Principal>, StoreError> {
        let row = sqlx::query("SELECT * FROM principals WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_principal).transpose()
    }

    pub async fn list_principals(&self) -> Result<Vec<Principal>, StoreError> {
        let rows = sqlx::query("SELECT * FROM principals ORDER BY created_at ASC, id ASC")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_principal).collect()
    }

    /// The principal a bearer token authenticates, or `None` when the token
    /// is unknown or the principal is disabled. Only the digest is compared.
    pub async fn authenticate(&self, token: &str) -> Result<Option<Principal>, StoreError> {
        let digest = token_hash(token);
        let row = sqlx::query("SELECT * FROM principals WHERE token_hash=?")
            .bind(&digest)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let stored: String = row.try_get("token_hash")?;
        if !crate::auth::constant_time_eq(&stored, &digest) {
            return Ok(None);
        }
        let principal = row_to_principal(&row)?;
        Ok((!principal.disabled).then_some(principal))
    }

    // -- web sessions --

    /// Start a web session for an already authenticated principal. Returns
    /// the opaque session id, the only thing the browser ever holds; the
    /// database keeps only its digest, so a copied database replays no
    /// session. Expired rows are purged on the way.
    pub async fn create_web_session(
        &self,
        principal: &Principal,
    ) -> Result<(String, DateTime<Utc>), StoreError> {
        let id = generate_session_id().map_err(StoreError::Storage)?;
        let created_at = now();
        let expires_at = created_at
            + chrono::Duration::from_std(WEB_SESSION_TTL)
                .map_err(|error| StoreError::Storage(error.to_string()))?;
        sqlx::query("DELETE FROM web_sessions WHERE expires_at <= ?")
            .bind(created_at)
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO web_sessions (id, principal_id, created_at, expires_at, revoked) VALUES (?,?,?,?,0)")
            .bind(token_hash(&id)).bind(&principal.id).bind(created_at).bind(expires_at)
            .execute(&self.pool).await?;
        self.record_event(
            "web_session.created",
            None,
            Some(&principal.id),
            serde_json::json!({"expires_at": expires_at}),
        )
        .await?;
        Ok((id, expires_at))
    }

    /// The principal behind a live web session: the session must exist,
    /// not be revoked, not be expired, and its principal must still be
    /// enabled. Anything else is anonymous.
    pub async fn authenticate_web_session(
        &self,
        session_id: &str,
    ) -> Result<Option<Principal>, StoreError> {
        if session_id.is_empty()
            || session_id.len() > 128
            || !session_id.starts_with(crate::auth::SESSION_PREFIX)
        {
            return Ok(None);
        }
        let row = sqlx::query("SELECT p.*, s.expires_at AS session_expires_at, s.revoked AS session_revoked FROM web_sessions s JOIN principals p ON p.id = s.principal_id WHERE s.id=?")
            .bind(token_hash(session_id))
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let revoked: i64 = row.try_get("session_revoked")?;
        let expires_at: DateTime<Utc> = row.try_get("session_expires_at")?;
        if revoked != 0 || expires_at <= now() {
            return Ok(None);
        }
        let principal = row_to_principal(&row)?;
        Ok((!principal.disabled).then_some(principal))
    }

    /// End a web session; a revoked session never authenticates again.
    pub async fn revoke_web_session(&self, session_id: &str) -> Result<(), StoreError> {
        let updated = sqlx::query("UPDATE web_sessions SET revoked=1 WHERE id=? AND revoked=0")
            .bind(token_hash(session_id))
            .execute(&self.pool)
            .await?;
        if updated.rows_affected() == 1 {
            self.record_event("web_session.revoked", None, None, serde_json::json!({}))
                .await?;
        }
        Ok(())
    }

    // -- events --

    async fn record_event(
        &self,
        kind: &str,
        skill_id: Option<&str>,
        principal_id: Option<&str>,
        payload: Value,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO events (kind, skill_id, principal_id, payload_json, created_at) VALUES (?,?,?,?,?)")
            .bind(kind).bind(skill_id).bind(principal_id).bind(payload.to_string()).bind(now())
            .execute(&self.pool).await?;
        Ok(())
    }

    async fn record_event_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        kind: &str,
        skill_id: Option<&str>,
        principal_id: Option<&str>,
        payload: Value,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO events (kind, skill_id, principal_id, payload_json, created_at) VALUES (?,?,?,?,?)")
            .bind(kind).bind(skill_id).bind(principal_id).bind(payload.to_string()).bind(now())
            .execute(&mut **transaction).await?;
        Ok(())
    }

    pub async fn events(&self, kind: Option<&str>) -> Result<Vec<Value>, StoreError> {
        let rows = sqlx::query("SELECT kind, skill_id, principal_id, payload_json, created_at FROM events WHERE (? IS NULL OR kind=?) ORDER BY sequence ASC")
            .bind(kind).bind(kind)
            .fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let payload: String = row.try_get("payload_json")?;
                Ok(serde_json::json!({
                    "kind": row.try_get::<String, _>("kind")?,
                    "skill_id": row.try_get::<Option<String>, _>("skill_id")?,
                    "principal_id": row.try_get::<Option<String>, _>("principal_id")?,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                }))
            })
            .collect()
    }

    // -- skills --

    /// Publish for the authenticated principal, as it stands inside the
    /// write transaction. The payload is validated independently here; the
    /// same team publishing the same content again receives the same skill
    /// (content addressing), so retries never create duplicates, and the
    /// same text under another sanitization version is a conflict.
    pub async fn publish(
        &self,
        publisher: &Principal,
        publication: &SkillPublication,
    ) -> Result<(SkillArtifact, bool), StoreError> {
        let publication = validate_publication(publication).map_err(StoreError::Invalid)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &publisher.id).await?;
        let publisher = &current;
        let existing = sqlx::query(
            "SELECT * FROM skills WHERE organization_id=? AND team_id=? AND content_digest=?",
        )
        .bind(&publisher.organization_id)
        .bind(&publisher.team_id)
        .bind(&publication.content_digest)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = existing {
            let skill = row_to_skill(&row)?;
            transaction.commit().await?;
            return Ok((skill, false));
        }
        Self::refuse_same_text_in(
            &mut transaction,
            &publisher.scope(),
            &publication.lesson,
            &publication.applicability,
        )
        .await?;
        let timestamp = now();
        let skill = SkillArtifact {
            id: new_id("skill"),
            status: SkillStatus::Candidate,
            visibility: publication.visibility,
            shared_scope: publisher.scope(),
            publisher: publisher.as_skill_principal(),
            lesson: publication.lesson.clone(),
            applicability: publication.applicability.clone(),
            content_digest: publication.content_digest.clone(),
            sanitization_version: publication.sanitization_version,
            version: publication.version.unwrap_or(1),
            parent_skill_id: publication.parent_skill_id.clone(),
            deprecation_reason: None,
            provenance: publication.provenance.clone(),
            created_at: timestamp,
            updated_at: timestamp,
            verified_at: None,
            deprecated_at: None,
            forked_by: None,
            responding_to_challenge_id: None,
            superseded_by: None,
            superseded_at: None,
            summary: None,
        };
        sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, deprecation_reason, parent_skill_id, version, provenance_json, created_at, updated_at, verified_at, deprecated_at, lineage_root_id, lineage_depth) VALUES (?,?,?,?,?,?,?,?,?,?,'candidate',NULL,?,?,?,?,?,NULL,NULL,?,0)")
            .bind(&skill.id).bind(&skill.shared_scope.organization_id).bind(&skill.shared_scope.team_id)
            .bind(&publisher.id).bind(&publisher.display_name).bind(skill.visibility.as_str())
            .bind(&skill.lesson).bind(&skill.applicability).bind(&skill.content_digest).bind(i64::from(skill.sanitization_version))
            .bind(&skill.parent_skill_id).bind(i64::from(skill.version))
            .bind(serde_json::to_string(&skill.provenance).unwrap_or_else(|_| "{}".into()))
            .bind(timestamp).bind(timestamp).bind(&skill.id)
            .execute(&mut *transaction).await?;
        Self::record_event_in(&mut transaction, "skill.published", Some(&skill.id), Some(&publisher.id), serde_json::json!({
            "content_digest": skill.content_digest, "visibility": skill.visibility, "sanitization_version": skill.sanitization_version,
        })).await?;
        transaction.commit().await?;
        Ok((skill, true))
    }

    async fn load_skill(&self, id: &str) -> Result<Option<SkillArtifact>, StoreError> {
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_to_skill).transpose()
    }

    /// A skill row that exists and can be read; an unreadable one is
    /// treated as absent. Storage failures are still errors.
    async fn load_readable_skill(&self, id: &str) -> Result<Option<SkillArtifact>, StoreError> {
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().and_then(|row| row_to_skill(row).ok()))
    }

    /// The digest binds the sanitization version, so the same text under
    /// another version is a different digest; a team still holds one skill
    /// per text, and the text itself is compared.
    async fn refuse_same_text_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        scope: &SharedScope,
        lesson: &str,
        applicability: &str,
    ) -> Result<(), StoreError> {
        let same: Option<String> = sqlx::query_scalar(
            "SELECT id FROM skills WHERE organization_id=? AND team_id=? AND lesson=? AND applicability=? AND typeof(id)='text' LIMIT 1",
        )
        .bind(&scope.organization_id)
        .bind(&scope.team_id)
        .bind(lesson)
        .bind(applicability)
        .fetch_optional(&mut **transaction)
        .await?;
        match same {
            Some(id) => Err(StoreError::Conflict(format!(
                "this text already exists in your team as skill {id}, under another sanitization version"
            ))),
            None => Ok(()),
        }
    }

    /// A skill the viewer may see, with its catalog summary.
    pub async fn get_skill(&self, viewer: &Viewer, id: &str) -> Result<SkillArtifact, StoreError> {
        let mut skill = self.load_skill(id).await?.ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        self.hide_invisible_links(viewer, &mut skill).await?;
        skill.summary = Some(self.aggregate(&skill.id).await?);
        Ok(skill)
    }

    /// A skill may name a parent or successor the viewer cannot see (a
    /// public fork of a public candidate, for instance). The link would
    /// reveal that the hidden skill exists, so it is removed from the
    /// viewer's copy, with the challenge on the hidden parent it answers.
    async fn hide_invisible_links(
        &self,
        viewer: &Viewer,
        skill: &mut SkillArtifact,
    ) -> Result<(), StoreError> {
        for link in [&mut skill.parent_skill_id, &mut skill.superseded_by] {
            if let Some(id) = link.clone() {
                // An unreadable linked row is treated as hidden, so it never
                // takes a listing or a page down with it.
                let visible = self
                    .load_readable_skill(&id)
                    .await?
                    .is_some_and(|linked| viewer.can_see(&linked));
                if !visible {
                    *link = None;
                }
            }
        }
        if skill.superseded_by.is_none() {
            skill.superseded_at = None;
        }
        if skill.parent_skill_id.is_none() {
            skill.responding_to_challenge_id = None;
        }
        Ok(())
    }

    /// The skills the viewer may see, newest first, filtered and paginated
    /// after the visibility rule so no hidden row leaks through counts.
    pub async fn list_skills(
        &self,
        viewer: &Viewer,
        filter: &ListFilter,
    ) -> Result<Vec<SkillArtifact>, StoreError> {
        let rows = sqlx::query("SELECT * FROM skills ORDER BY created_at DESC, id DESC")
            .fetch_all(&self.pool)
            .await?;
        let needle = filter.q.as_ref().map(|q| q.to_lowercase());
        let mut visible = Vec::new();
        for row in &rows {
            // One unreadable row never takes the catalog down with it.
            let Ok(skill) = row_to_skill(row) else {
                continue;
            };
            if !viewer.can_see(&skill) {
                continue;
            }
            if filter.status.is_some_and(|status| skill.status != status) {
                continue;
            }
            if let Some(needle) = &needle
                && !skill.lesson.to_lowercase().contains(needle)
                && !skill.applicability.to_lowercase().contains(needle)
                && !skill.id.to_lowercase().contains(needle)
            {
                continue;
            }
            visible.push(skill);
        }
        let limit = filter
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .clamp(1, MAX_LIST_LIMIT) as usize;
        let offset = filter.offset.unwrap_or(0) as usize;
        let mut page = Vec::new();
        for mut skill in visible {
            let summary = self.aggregate(&skill.id).await?;
            if filter
                .task_family
                .as_ref()
                .is_some_and(|family| !summary.task_families.iter().any(|f| f == family))
                || filter
                    .model
                    .as_ref()
                    .is_some_and(|model| !summary.models.iter().any(|m| m == model))
            {
                continue;
            }
            skill.summary = Some(summary);
            page.push(skill);
        }
        let mut page: Vec<SkillArtifact> = page.into_iter().skip(offset).take(limit).collect();
        for skill in &mut page {
            self.hide_invisible_links(viewer, skill).await?;
        }
        Ok(page)
    }

    pub async fn list_receipts(
        &self,
        viewer: &Viewer,
        skill_id: &str,
    ) -> Result<Vec<SkillReceiptItem>, StoreError> {
        let skill = self
            .load_skill(skill_id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        let rows = sqlx::query(RECEIPTS_NEWEST_FIRST)
            .bind(skill_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row_to_receipt(row).ok())
            .collect())
    }

    /// The receipts the gate reads, and whether every one of them could be
    /// read: the gate never verifies on evidence it cannot read.
    async fn summaries_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        skill_id: &str,
    ) -> Result<(Vec<ReceiptSummary>, bool), StoreError> {
        let rows = sqlx::query(RECEIPTS_OLDEST_FIRST)
            .bind(skill_id)
            .fetch_all(&mut **transaction)
            .await?;
        let items: Vec<SkillReceiptItem> = rows
            .iter()
            .filter_map(|row| row_to_receipt(row).ok())
            .collect();
        let readable = items.len() == rows.len();
        Ok((items.iter().map(ReceiptSummary::from).collect(), readable))
    }

    pub async fn aggregate(&self, skill_id: &str) -> Result<SkillAggregate, StoreError> {
        let rows = sqlx::query(RECEIPTS_OLDEST_FIRST)
            .bind(skill_id)
            .fetch_all(&self.pool)
            .await?;
        let summaries: Vec<ReceiptSummary> = rows
            .iter()
            .filter_map(|row| row_to_receipt(row).ok())
            .map(|item| ReceiptSummary::from(&item))
            .collect();
        Ok(aggregate_receipts(&summaries))
    }

    /// Append one immutable receipt for the authenticated evaluator and apply
    /// the domain gate in the same write transaction.
    pub async fn record_receipt(
        &self,
        evaluator: &Principal,
        skill_id: &str,
        submission: &SkillReceiptSubmission,
    ) -> Result<RecordedReceipt, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &evaluator.id).await?;
        let evaluator = &current;
        let viewer = Viewer {
            principal: Some(evaluator.clone()),
        };
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(skill_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let mut skill = row_to_skill(&row)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        let protocol_digest = validate_receipt(submission, &skill.id, &skill.content_digest)
            .map_err(|message| StoreError::Invalid(format!("skill receipt rejected: {message}")))?;
        let duplicates: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipts WHERE skill_id=? AND principal_id=? AND protocol_digest=?")
            .bind(&skill.id).bind(&evaluator.id).bind(&protocol_digest)
            .fetch_one(&mut *transaction).await?;
        if duplicates > 0 {
            return Err(StoreError::Conflict(
                "a receipt with this protocol is already recorded for this principal; receipts are immutable".into(),
            ));
        }
        let (verdict, complete, safety) = receipt_verdict(submission);
        let result = serde_json::to_string(submission)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if result.len() > MAX_RESULT_BYTES {
            return Err(StoreError::Invalid("the receipt is too large".into()));
        }
        let timestamp = now();
        // Authority is decided here, from server-side facts only: a member of
        // the skill's team, or a principal the administrator authorized to
        // evaluate other teams' public skills. Everyone else who can see the
        // skill may still file a community receipt.
        let authoritative = viewer.in_team(
            &skill.shared_scope.organization_id,
            &skill.shared_scope.team_id,
        ) || evaluator.authorized_evaluator;
        let receipt = SkillReceiptItem {
            id: new_id("receipt"),
            skill_id: skill.id.clone(),
            evaluator: evaluator.as_skill_principal(),
            evaluator_program: serde_json::to_value(&submission.evaluator).unwrap_or(Value::Null),
            independent: evaluator.id != skill.publisher.id,
            authoritative,
            origin: "direct".into(),
            protocol_version: submission.protocol_version,
            protocol_digest,
            complete,
            safety,
            task_family: submission.task_family.clone(),
            model: Some(submission.model.clone()),
            verdict,
            created_at: timestamp,
        };
        let inserted = sqlx::query("INSERT INTO receipts (id, skill_id, principal_id, principal_display, evaluator_json, independent, authoritative, protocol_version, protocol_digest, complete, safety, task_family, model, verdict_json, result_json, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&receipt.id).bind(&receipt.skill_id).bind(&evaluator.id).bind(&evaluator.display_name)
            .bind(receipt.evaluator_program.to_string()).bind(i64::from(receipt.independent)).bind(i64::from(receipt.authoritative)).bind(i64::from(receipt.protocol_version))
            .bind(&receipt.protocol_digest).bind(i64::from(receipt.complete)).bind(receipt.safety.as_str())
            .bind(&receipt.task_family).bind(&receipt.model).bind(receipt.verdict.to_string()).bind(&result).bind(timestamp)
            .execute(&mut *transaction).await;
        match inserted {
            Ok(_) => {}
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                return Err(StoreError::Conflict(
                    "a receipt with this protocol is already recorded for this principal; receipts are immutable".into(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let (summaries, readable) = Self::summaries_in(&mut transaction, &skill.id).await?;
        let requested = match skill_gate(skill.status, &summaries) {
            SkillTransition::Verify if !readable => SkillTransition::None,
            requested => requested,
        };
        let transition = match &requested {
            SkillTransition::None => SkillTransition::None,
            SkillTransition::Verify => {
                let updated = sqlx::query("UPDATE skills SET status='verified', verified_at=?, updated_at=? WHERE id=? AND status='candidate'")
                    .bind(timestamp).bind(timestamp).bind(&skill.id)
                    .execute(&mut *transaction).await?;
                if updated.rows_affected() != 1 {
                    return Err(StoreError::Conflict(format!(
                        "skill {} changed while its receipt was being recorded; nothing was stored",
                        skill.id
                    )));
                }
                skill.status = SkillStatus::Verified;
                skill.verified_at = Some(timestamp);
                skill.updated_at = timestamp;
                SkillTransition::Verify
            }
            SkillTransition::Deprecate(reason) => {
                let updated = sqlx::query("UPDATE skills SET status='deprecated', deprecation_reason=?, deprecated_at=?, updated_at=? WHERE id=? AND status IN ('candidate','verified')")
                    .bind(reason).bind(timestamp).bind(timestamp).bind(&skill.id)
                    .execute(&mut *transaction).await?;
                if updated.rows_affected() == 1 {
                    skill.status = SkillStatus::Deprecated;
                    skill.deprecation_reason = Some(reason.clone());
                    skill.deprecated_at = Some(timestamp);
                    skill.updated_at = timestamp;
                    SkillTransition::Deprecate(reason.clone())
                } else {
                    SkillTransition::None
                }
            }
        };
        Self::record_event_in(&mut transaction, "skill.evaluated", Some(&skill.id), Some(&evaluator.id), serde_json::json!({
            "receipt_id": receipt.id, "independent": receipt.independent, "authoritative": receipt.authoritative, "complete": receipt.complete, "safety": receipt.safety,
            "protocol_digest": receipt.protocol_digest, "transition": transition.label(),
        })).await?;
        match &transition {
            SkillTransition::Verify => {
                Self::record_event_in(&mut transaction, "skill.verified", Some(&skill.id), None, serde_json::json!({
                    "decided_by": "gate", "gate_version": SKILL_GATE_VERSION,
                    "independent_evaluators": aggregate_receipts(&summaries).independent_evaluators,
                })).await?;
            }
            SkillTransition::Deprecate(reason) => {
                Self::record_event_in(
                    &mut transaction,
                    "skill.deprecated",
                    Some(&skill.id),
                    None,
                    serde_json::json!({
                        "decided_by": "gate", "gate_version": SKILL_GATE_VERSION, "reason": reason,
                    }),
                )
                .await?;
            }
            SkillTransition::None => {}
        }
        // A fork that just became verified may now pass the supersession
        // gate with comparisons recorded before it was verified; a skill that
        // was just deprecated releases its predecessor if it was the
        // successor.
        match &transition {
            SkillTransition::Verify => {
                if skill.forked_by.is_some()
                    && let Some(parent_id) = skill.parent_skill_id.clone()
                {
                    Self::settle_supersession_in(
                        &mut transaction,
                        &parent_id,
                        Some(&skill.id),
                        timestamp,
                    )
                    .await?;
                }
            }
            SkillTransition::Deprecate(_) => {
                Self::release_predecessor_in(
                    &mut transaction,
                    &skill.id,
                    "successor_deprecated",
                    timestamp,
                )
                .await?;
            }
            SkillTransition::None => {}
        }
        transaction.commit().await?;
        skill.summary = Some(self.aggregate(&skill.id).await?);
        Ok(RecordedReceipt {
            receipt,
            skill,
            transition,
        })
    }

    // -- forum: forks and lineage --

    /// Fork a skill the forker can see into a new candidate. The parent is
    /// the path, the version is the parent's plus one, the scope is the
    /// forker's own team, and the visibility is inherited when the forker
    /// belongs to the parent's team and public otherwise, so a fork of a
    /// public skill by another team stays comparable by everyone. The
    /// content must differ from the parent's; identical content in the
    /// forker's team is the same fork (retry) or a conflict (a different
    /// skill already holds it, also under another sanitization version).
    /// The forker is judged as it stands inside the write transaction. The
    /// parent is never edited.
    pub async fn create_fork(
        &self,
        forker: &Principal,
        parent_id: &str,
        submission: &ForkSubmission,
    ) -> Result<(SkillArtifact, bool), StoreError> {
        let submission = validate_fork(submission).map_err(StoreError::Invalid)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &forker.id).await?;
        let forker = &current;
        let viewer = Viewer {
            principal: Some(forker.clone()),
        };
        let parent_row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(parent_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let parent = row_to_skill(&parent_row)?;
        if !viewer.can_see(&parent) {
            return Err(StoreError::NotFound);
        }
        // The digest binds the sanitization version, so the text itself is
        // compared too: relabelling the parent's text is no refinement.
        if parent.content_digest == submission.content_digest
            || (parent.lesson == submission.lesson
                && parent.applicability == submission.applicability)
        {
            return Err(StoreError::Invalid(
                "a fork must change the lesson or the applicability".into(),
            ));
        }
        if let Some(challenge_id) = &submission.responding_to_challenge_id {
            let known: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM challenges WHERE id=? AND skill_id=?")
                    .bind(challenge_id)
                    .bind(&parent.id)
                    .fetch_one(&mut *transaction)
                    .await?;
            if known != 1 {
                return Err(StoreError::Invalid(
                    "responding_to_challenge_id must name a challenge recorded on the parent"
                        .into(),
                ));
            }
        }
        let in_parent_team = viewer.in_team(
            &parent.shared_scope.organization_id,
            &parent.shared_scope.team_id,
        );
        let visibility = if in_parent_team {
            parent.visibility
        } else {
            SkillVisibility::Public
        };
        let existing = sqlx::query(
            "SELECT * FROM skills WHERE organization_id=? AND team_id=? AND content_digest=?",
        )
        .bind(&forker.organization_id)
        .bind(&forker.team_id)
        .bind(&submission.content_digest)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = existing {
            let skill = row_to_skill(&row)?;
            transaction.commit().await?;
            return if skill.parent_skill_id.as_deref() == Some(parent.id.as_str()) {
                Ok((skill, false))
            } else {
                Err(StoreError::Conflict(format!(
                    "this content already exists in your team as skill {}",
                    skill.id
                )))
            };
        }
        Self::refuse_same_text_in(
            &mut transaction,
            &forker.scope(),
            &submission.lesson,
            &submission.applicability,
        )
        .await?;
        // Bounds, checked in the same write transaction as the insert: the
        // depth below the root, and quotas per forking team for the
        // parent's direct forks and for the whole tree. Other teams share
        // what is left beside the owners' own quota, so one outsider can
        // never use up the room of the skill's or the tree's owners, and
        // every lineage answer and page stays small.
        let bounds = sqlx::query(
            "SELECT COALESCE(lineage_root_id, id) AS root, lineage_depth FROM skills WHERE id=?",
        )
        .bind(&parent.id)
        .fetch_one(&mut *transaction)
        .await?;
        let tree_root: String = bounds.try_get("root")?;
        let depth = bounds.try_get::<i64, _>("lineage_depth")?.saturating_add(1);
        if depth > i64::from(MAX_LINEAGE_DEPTH) {
            return Err(StoreError::Conflict(format!(
                "a lineage may be at most {MAX_LINEAGE_DEPTH} forks deep"
            )));
        }
        let quota = |limit: usize| i64::try_from(limit).unwrap_or(i64::MAX);
        let team_forks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM skills WHERE parent_skill_id=? AND forked_by_id IS NOT NULL AND organization_id=? AND team_id=?",
        )
        .bind(&parent.id)
        .bind(&forker.organization_id)
        .bind(&forker.team_id)
        .fetch_one(&mut *transaction)
        .await?;
        if team_forks >= quota(MAX_FORKS_PER_TEAM_PER_SKILL) {
            return Err(StoreError::Conflict(format!(
                "a team may fork one skill at most {MAX_FORKS_PER_TEAM_PER_SKILL} times"
            )));
        }
        if !in_parent_team {
            let other_forks: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM skills WHERE parent_skill_id=? AND forked_by_id IS NOT NULL AND NOT (organization_id=? AND team_id=?)",
            )
            .bind(&parent.id)
            .bind(&parent.shared_scope.organization_id)
            .bind(&parent.shared_scope.team_id)
            .fetch_one(&mut *transaction)
            .await?;
            if other_forks >= quota(MAX_FORKS_PER_SKILL) {
                return Err(StoreError::Conflict(format!(
                    "a skill takes at most {MAX_FORKS_PER_SKILL} forks from teams other than its own"
                )));
            }
        }
        let root_scope = sqlx::query("SELECT organization_id, team_id FROM skills WHERE id=?")
            .bind(&tree_root)
            .fetch_one(&mut *transaction)
            .await?;
        let root_organization: String = root_scope.try_get("organization_id")?;
        let root_team: String = root_scope.try_get("team_id")?;
        let team_tree: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM skills WHERE lineage_root_id=? AND forked_by_id IS NOT NULL AND organization_id=? AND team_id=?",
        )
        .bind(&tree_root)
        .bind(&forker.organization_id)
        .bind(&forker.team_id)
        .fetch_one(&mut *transaction)
        .await?;
        if team_tree >= quota(MAX_FORKS_PER_TEAM_PER_LINEAGE) {
            return Err(StoreError::Conflict(format!(
                "a team may create at most {MAX_FORKS_PER_TEAM_PER_LINEAGE} forks in one lineage"
            )));
        }
        if !(forker.organization_id == root_organization && forker.team_id == root_team) {
            let other_tree: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM skills WHERE lineage_root_id=? AND forked_by_id IS NOT NULL AND NOT (organization_id=? AND team_id=?)",
            )
            .bind(&tree_root)
            .bind(&root_organization)
            .bind(&root_team)
            .fetch_one(&mut *transaction)
            .await?;
            let shared = MAX_LINEAGE_NODES - 1 - MAX_FORKS_PER_TEAM_PER_LINEAGE;
            if other_tree >= quota(shared) {
                return Err(StoreError::Conflict(format!(
                    "a lineage takes at most {shared} forks from teams other than its root's"
                )));
            }
        }
        let timestamp = now();
        let skill = SkillArtifact {
            id: new_id("skill"),
            status: SkillStatus::Candidate,
            visibility,
            shared_scope: forker.scope(),
            publisher: forker.as_skill_principal(),
            lesson: submission.lesson.clone(),
            applicability: submission.applicability.clone(),
            content_digest: submission.content_digest.clone(),
            sanitization_version: submission.sanitization_version,
            version: parent.version.saturating_add(1),
            parent_skill_id: Some(parent.id.clone()),
            deprecation_reason: None,
            provenance: submission.provenance.clone(),
            created_at: timestamp,
            updated_at: timestamp,
            verified_at: None,
            deprecated_at: None,
            forked_by: Some(forker.as_skill_principal()),
            responding_to_challenge_id: submission.responding_to_challenge_id.clone(),
            superseded_by: None,
            superseded_at: None,
            summary: None,
        };
        sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, deprecation_reason, parent_skill_id, version, provenance_json, created_at, updated_at, verified_at, deprecated_at, forked_by_id, forked_by_display, responding_to_challenge_id, lineage_root_id, lineage_depth) VALUES (?,?,?,?,?,?,?,?,?,?,'candidate',NULL,?,?,?,?,?,NULL,NULL,?,?,?,?,?)")
            .bind(&skill.id).bind(&skill.shared_scope.organization_id).bind(&skill.shared_scope.team_id)
            .bind(&forker.id).bind(&forker.display_name).bind(skill.visibility.as_str())
            .bind(&skill.lesson).bind(&skill.applicability).bind(&skill.content_digest).bind(i64::from(skill.sanitization_version))
            .bind(&skill.parent_skill_id).bind(i64::from(skill.version))
            .bind(serde_json::to_string(&skill.provenance).unwrap_or_else(|_| "{}".into()))
            .bind(timestamp).bind(timestamp)
            .bind(&forker.id).bind(&forker.display_name).bind(&skill.responding_to_challenge_id)
            .bind(&tree_root).bind(depth)
            .execute(&mut *transaction).await?;
        Self::record_event_in(&mut transaction, "skill.forked", Some(&skill.id), Some(&forker.id), serde_json::json!({
            "parent_skill_id": parent.id, "version": skill.version, "visibility": skill.visibility,
            "content_digest": skill.content_digest, "responding_to_challenge_id": skill.responding_to_challenge_id,
        })).await?;
        transaction.commit().await?;
        Ok((skill, true))
    }

    /// The direct forks of a skill the viewer can see, oldest first, each
    /// with its summary, filtered by the viewer's visibility.
    pub async fn list_forks(
        &self,
        viewer: &Viewer,
        parent_id: &str,
    ) -> Result<Vec<SkillArtifact>, StoreError> {
        let parent = self
            .load_skill(parent_id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&parent) {
            return Err(StoreError::NotFound);
        }
        let rows = sqlx::query(
            "SELECT * FROM skills WHERE parent_skill_id=? AND forked_by_id IS NOT NULL ORDER BY created_at ASC, id ASC LIMIT ?",
        )
        .bind(parent_id)
        .bind(i64::try_from(MAX_FORKS_PER_SKILL + MAX_FORKS_PER_TEAM_PER_SKILL).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        let mut forks = Vec::new();
        for row in &rows {
            let Ok(mut fork) = row_to_skill(row) else {
                continue;
            };
            if viewer.can_see(&fork) {
                self.hide_invisible_links(viewer, &mut fork).await?;
                fork.summary = Some(self.aggregate(&fork.id).await?);
                forks.push(fork);
            }
        }
        Ok(forks)
    }

    /// The lineage tree containing a skill the viewer can see, read in one
    /// bounded query (at most `MAX_LINEAGE_NODES` skills, shallowest first):
    /// every node the viewer may see, with challenge counts from one grouped
    /// query, and the active version of the requested skill. The active
    /// version is computed over the whole tree and reported only when the
    /// viewer may see it. A link to a parent or successor the viewer cannot
    /// see is removed, and the reported root is the highest ancestor the
    /// viewer can see, so the answer never reveals a hidden skill.
    pub async fn lineage(&self, viewer: &Viewer, id: &str) -> Result<Lineage, StoreError> {
        let requested = self.load_skill(id).await?.ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&requested) {
            return Err(StoreError::NotFound);
        }
        let tree_root: String =
            sqlx::query_scalar("SELECT COALESCE(lineage_root_id, id) FROM skills WHERE id=?")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        let rows = sqlx::query(
            "SELECT * FROM skills WHERE lineage_root_id=? OR id=? ORDER BY lineage_depth ASC, created_at ASC, id ASC LIMIT ?",
        )
        .bind(&tree_root)
        .bind(&tree_root)
        .bind(i64::try_from(MAX_LINEAGE_NODES).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        let mut skills: Vec<SkillArtifact> = rows
            .iter()
            .filter_map(|row| row_to_skill(row).ok())
            .collect();
        if !skills.iter().any(|skill| skill.id == requested.id) {
            skills.push(requested.clone());
        }
        let visible: std::collections::BTreeSet<String> = skills
            .iter()
            .filter(|skill| viewer.can_see(skill))
            .map(|skill| skill.id.clone())
            .collect();
        let count_rows = sqlx::query(
            "SELECT skill_id, SUM(CASE WHEN status='open' THEN 1 ELSE 0 END) AS open_count, SUM(CASE WHEN evidence_receipt_id IS NOT NULL THEN 1 ELSE 0 END) AS backed_count FROM challenges WHERE skill_id IN (SELECT id FROM skills WHERE lineage_root_id=? OR id=?) GROUP BY skill_id",
        )
        .bind(&tree_root)
        .bind(&tree_root)
        .fetch_all(&self.pool)
        .await?;
        let mut counts = std::collections::BTreeMap::new();
        for row in &count_rows {
            let skill_id: String = row.try_get("skill_id")?;
            let open: i64 = row.try_get("open_count")?;
            let backed: i64 = row.try_get("backed_count")?;
            counts.insert(skill_id, (open, backed));
        }
        let all_nodes: Vec<LineageNode> = skills
            .iter()
            .map(|skill| {
                let (open, backed) = counts.get(&skill.id).copied().unwrap_or((0, 0));
                LineageNode {
                    id: skill.id.clone(),
                    version: skill.version,
                    status: skill.status,
                    visibility: skill.visibility,
                    parent_skill_id: skill.parent_skill_id.clone(),
                    publisher: skill.publisher.clone(),
                    forked_by: skill.forked_by.clone(),
                    responding_to_challenge_id: skill.responding_to_challenge_id.clone(),
                    superseded_by: skill.superseded_by.clone(),
                    verified_at: skill.verified_at,
                    deprecated_at: skill.deprecated_at,
                    open_challenges: u32::try_from(open).unwrap_or(u32::MAX),
                    evidence_backed_challenges: u32::try_from(backed).unwrap_or(u32::MAX),
                }
            })
            .collect();
        let active_id =
            active_successor(&all_nodes, &requested.id).filter(|active| visible.contains(active));
        let nodes: Vec<LineageNode> = all_nodes
            .into_iter()
            .filter(|node| visible.contains(&node.id))
            .map(|mut node| {
                if node
                    .parent_skill_id
                    .as_ref()
                    .is_some_and(|parent| !visible.contains(parent))
                {
                    node.parent_skill_id = None;
                    node.responding_to_challenge_id = None;
                }
                if node
                    .superseded_by
                    .as_ref()
                    .is_some_and(|successor| !visible.contains(successor))
                {
                    node.superseded_by = None;
                }
                node
            })
            .collect();
        let parent_of: std::collections::BTreeMap<&str, &str> = nodes
            .iter()
            .filter_map(|node| {
                node.parent_skill_id
                    .as_deref()
                    .map(|parent| (node.id.as_str(), parent))
            })
            .collect();
        let mut root_id = requested.id.as_str();
        for _ in 0..=nodes.len() {
            match parent_of.get(root_id) {
                Some(parent) => root_id = parent,
                None => break,
            }
        }
        let root_id = root_id.to_owned();
        Ok(Lineage {
            requested_id: requested.id,
            root_id,
            active_id,
            nodes,
        })
    }

    // -- forum: comparative evaluation and supersession --

    /// The one team that owns the fork, the parent and every skill of the
    /// parent's supersession chain, when there is one. Such a lineage
    /// answers to that team alone; a lineage that holds skills of several
    /// teams answers only to authorized evaluators, so no team can move a
    /// chain that also answers for another team.
    async fn lineage_team(
        connection: &mut sqlx::SqliteConnection,
        parent: &SkillArtifact,
        fork: &SkillArtifact,
    ) -> Result<Option<SharedScope>, StoreError> {
        let scope = |skill: &SkillArtifact| {
            (
                skill.shared_scope.organization_id.clone(),
                skill.shared_scope.team_id.clone(),
            )
        };
        let mut teams = std::collections::BTreeSet::from([scope(parent), scope(fork)]);
        let mut current = parent.id.clone();
        for _ in 0..=MAX_LINEAGE_DEPTH {
            let Some(row) = sqlx::query(
                "SELECT id, organization_id, team_id FROM skills WHERE superseded_by=? LIMIT 1",
            )
            .bind(&current)
            .fetch_optional(&mut *connection)
            .await?
            else {
                break;
            };
            // A predecessor that cannot be read counts as another team's.
            let (Ok(organization_id), Ok(team_id), Ok(id)) = (
                row.try_get::<String, _>("organization_id"),
                row.try_get::<String, _>("team_id"),
                row.try_get::<String, _>("id"),
            ) else {
                return Ok(None);
            };
            teams.insert((organization_id, team_id));
            current = id;
        }
        if teams.len() != 1 {
            return Ok(None);
        }
        Ok(teams
            .into_iter()
            .next()
            .map(|(organization_id, team_id)| SharedScope {
                organization_id,
                team_id,
            }))
    }

    /// Every comparison of a fork, oldest first, with live authorities and
    /// the safety vetoes recorded at submission; `None` when any of them
    /// cannot be read. The gate never passes on evidence it cannot read,
    /// since an unreadable row may be the veto that blocks the fork.
    async fn comparison_summaries_in(
        connection: &mut sqlx::SqliteConnection,
        parent: &SkillArtifact,
        fork: &SkillArtifact,
    ) -> Result<Option<Vec<ComparisonSummary>>, StoreError> {
        let team = Self::lineage_team(connection, parent, fork).await?;
        let rows = sqlx::query(&format!(
            "{COMPARISONS_SELECT} ORDER BY c.created_at ASC, c.id ASC"
        ))
        .bind(&fork.id)
        .fetch_all(&mut *connection)
        .await?;
        let mut summaries = Vec::with_capacity(rows.len());
        for row in &rows {
            let (Ok(item), Ok(recorded_link), Ok(recorded_parent)) = (
                row_to_comparison(row, team.as_ref()),
                row.try_get::<i64, _>("authoritative"),
                row.try_get::<i64, _>("parent_authority"),
            ) else {
                return Ok(None);
            };
            let mut summary = ComparisonSummary::from(&item);
            summary.blocks_fork = item.fork_safety == SkillSafety::Failed
                && (recorded_link != 0 || recorded_parent != 0);
            summaries.push(summary);
        }
        Ok(Some(summaries))
    }

    /// One page of the comparisons recorded for a fork the viewer can see,
    /// newest first, with live authorities.
    pub async fn list_comparisons(
        &self,
        viewer: &Viewer,
        fork_id: &str,
        page: &ForumPage,
    ) -> Result<Vec<ComparisonItem>, StoreError> {
        let fork = self
            .load_skill(fork_id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&fork) {
            return Err(StoreError::NotFound);
        }
        let Some(parent_id) = fork.parent_skill_id.clone() else {
            return Ok(Vec::new());
        };
        let parent_row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(&parent_id)
            .fetch_optional(&self.pool)
            .await?;
        let mut connection = self.pool.acquire().await?;
        // A parent that is missing or unreadable belongs to no team the link
        // could answer to; the fork's own comparisons still list.
        let team = match parent_row.as_ref().map(row_to_skill) {
            Some(Ok(parent)) => Self::lineage_team(&mut connection, &parent, &fork).await?,
            _ => None,
        };
        if let Some(before) = &page.before {
            let known: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM comparisons WHERE id=? AND fork_id=?")
                    .bind(before)
                    .bind(fork_id)
                    .fetch_one(&mut *connection)
                    .await?;
            if known != 1 {
                return Err(StoreError::Invalid(
                    "before must name a comparison of this fork".into(),
                ));
            }
        }
        let cursor = if page.before.is_some() {
            " AND (c.created_at, c.id) < (SELECT created_at, id FROM comparisons WHERE id=?)"
        } else {
            ""
        };
        let sql =
            format!("{COMPARISONS_SELECT}{cursor} ORDER BY c.created_at DESC, c.id DESC LIMIT ?");
        let mut query = sqlx::query(&sql).bind(fork_id);
        if let Some(before) = &page.before {
            query = query.bind(before);
        }
        let rows = query
            .bind(i64::from(list_limit(page.limit)))
            .fetch_all(&mut *connection)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row_to_comparison(row, team.as_ref()).ok())
            .collect())
    }

    /// Append one immutable parent-versus-fork comparison for the
    /// authenticated evaluator, apply its safety evidence and settle the
    /// supersession gate, all in one write transaction.
    ///
    /// - Only a fork created through the forks endpoint can be compared, only
    ///   against a verified parent, and only while the fork is not deprecated.
    ///   One principal records at most `MAX_COMPARISONS_PER_PRINCIPAL`
    ///   comparisons of one fork.
    /// - Three authorities are decided here from server-side facts only.
    ///   Link authority counts toward the gate: an authorized evaluator, or a
    ///   member of the one team that owns the whole lineage (see
    ///   `lineage_team`). Parent authority speaks for the parent: an
    ///   authorized evaluator or a member of the parent's team. Fork
    ///   authority speaks for the fork: an authorized evaluator or a member
    ///   of the fork's team. A comparison is independent when the evaluator
    ///   published neither skill.
    /// - Safety evidence: a failed fork arm deprecates the fork under fork
    ///   authority; under link or parent authority it blocks the fork for
    ///   good and withdraws the parent's link to it. A failed parent arm
    ///   deprecates the parent under parent authority. Every deprecation
    ///   releases the deprecated skill's predecessor.
    /// - An installed link holds only while its gate holds: a newer
    ///   comparison that makes the gate fail withdraws it. Otherwise the gate
    ///   is settled with a compare-and-set on the parent, so two forks
    ///   competing for one parent produce exactly one successor.
    pub async fn record_comparison(
        &self,
        evaluator: &Principal,
        fork_id: &str,
        submission: &ComparisonSubmission,
    ) -> Result<
        (
            ComparisonItem,
            SkillArtifact,
            SkillArtifact,
            SupersessionTransition,
        ),
        StoreError,
    > {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &evaluator.id).await?;
        let evaluator = &current;
        let viewer = Viewer {
            principal: Some(evaluator.clone()),
        };
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(fork_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let fork = row_to_skill(&row)?;
        if !viewer.can_see(&fork) {
            return Err(StoreError::NotFound);
        }
        let parent_id = fork
            .parent_skill_id
            .clone()
            .filter(|_| fork.forked_by.is_some())
            .ok_or_else(|| {
                StoreError::Invalid(
                    "comparisons target a fork created through the forks endpoint".into(),
                )
            })?;
        let parent_row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(&parent_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let parent = row_to_skill(&parent_row)?;
        if !viewer.can_see(&parent) {
            return Err(StoreError::NotFound);
        }
        let protocol_digest = validate_comparison(
            submission,
            &fork.id,
            &fork.content_digest,
            &parent.id,
            &parent.content_digest,
        )
        .map_err(|message| StoreError::Invalid(format!("comparison rejected: {message}")))?;
        if parent.status != SkillStatus::Verified {
            return Err(StoreError::Conflict(
                "comparisons need a verified, undeprecated parent".into(),
            ));
        }
        if fork.status == SkillStatus::Deprecated {
            return Err(StoreError::Conflict(
                "the fork is deprecated; it can no longer supersede its parent".into(),
            ));
        }
        let duplicates: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM comparisons WHERE fork_id=? AND principal_id=? AND protocol_digest=?")
            .bind(&fork.id).bind(&evaluator.id).bind(&protocol_digest)
            .fetch_one(&mut *transaction).await?;
        if duplicates > 0 {
            return Err(StoreError::Conflict(
                "a comparison with this protocol is already recorded for this principal; comparisons are immutable".into(),
            ));
        }
        let recorded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM comparisons WHERE fork_id=? AND principal_id=?",
        )
        .bind(&fork.id)
        .bind(&evaluator.id)
        .fetch_one(&mut *transaction)
        .await?;
        if recorded >= i64::try_from(MAX_COMPARISONS_PER_PRINCIPAL).unwrap_or(i64::MAX) {
            return Err(StoreError::Conflict(format!(
                "one principal may record at most {MAX_COMPARISONS_PER_PRINCIPAL} comparisons of one fork"
            )));
        }
        let (verdict, complete, fork_safety) = comparison_verdict(submission);
        let result = serde_json::to_string(submission)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        if result.len() > MAX_RESULT_BYTES {
            return Err(StoreError::Invalid("the comparison is too large".into()));
        }
        let timestamp = now();
        let in_scope = |scope: &SharedScope| viewer.in_team(&scope.organization_id, &scope.team_id);
        let lineage_team = Self::lineage_team(&mut transaction, &parent, &fork).await?;
        let over_link =
            evaluator.authorized_evaluator || lineage_team.as_ref().is_some_and(in_scope);
        let over_parent = evaluator.authorized_evaluator || in_scope(&parent.shared_scope);
        let over_fork = evaluator.authorized_evaluator || in_scope(&fork.shared_scope);
        let comparison = ComparisonItem {
            id: new_id("comparison"),
            fork_id: fork.id.clone(),
            parent_id: parent.id.clone(),
            evaluator: evaluator.as_skill_principal(),
            evaluator_program: serde_json::to_value(&submission.evaluator).unwrap_or(Value::Null),
            independent: evaluator.id != fork.publisher.id && evaluator.id != parent.publisher.id,
            authoritative: over_link,
            parent_authority: over_parent,
            protocol_version: submission.protocol_version,
            protocol_digest,
            complete,
            fork_safety,
            task_family: submission.task_family.clone(),
            model: Some(submission.model.clone()),
            verdict,
            created_at: timestamp,
        };
        let inserted = sqlx::query("INSERT INTO comparisons (id, fork_id, parent_id, principal_id, principal_display, evaluator_json, independent, authoritative, parent_authority, protocol_version, protocol_digest, complete, fork_safety, task_family, model, verdict_json, result_json, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&comparison.id).bind(&comparison.fork_id).bind(&comparison.parent_id).bind(&evaluator.id).bind(&evaluator.display_name)
            .bind(comparison.evaluator_program.to_string()).bind(i64::from(comparison.independent)).bind(i64::from(comparison.authoritative))
            .bind(i64::from(comparison.parent_authority)).bind(i64::from(comparison.protocol_version)).bind(&comparison.protocol_digest)
            .bind(i64::from(comparison.complete)).bind(comparison.fork_safety.as_str()).bind(&comparison.task_family).bind(&comparison.model)
            .bind(comparison.verdict.to_string()).bind(&result).bind(timestamp)
            .execute(&mut *transaction).await;
        match inserted {
            Ok(_) => {}
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                return Err(StoreError::Conflict(
                    "a comparison with this protocol is already recorded for this principal; comparisons are immutable".into(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let evidence =
            serde_json::json!({"comparison_id": comparison.id, "evaluator_id": evaluator.id});
        // Both deprecations the evidence calls for are applied before any
        // link is released, so no settlement runs on a parent this same
        // comparison condemns.
        let fork_deprecated = fork_safety == SkillSafety::Failed
            && over_fork
            && Self::deprecate_in(
                &mut transaction,
                &fork.id,
                COMPARISON_SAFETY_REASON,
                &evidence,
                timestamp,
            )
            .await?;
        let parent_deprecated = submission.parent_safety.verdict == PoisoningVerdict::Leaked
            && over_parent
            && Self::deprecate_in(
                &mut transaction,
                &parent.id,
                COMPARISON_SAFETY_REASON,
                &evidence,
                timestamp,
            )
            .await?;
        for (deprecated, id) in [(fork_deprecated, &fork.id), (parent_deprecated, &parent.id)] {
            if deprecated {
                Self::release_predecessor_in(
                    &mut transaction,
                    id,
                    "successor_deprecated",
                    timestamp,
                )
                .await?;
            }
        }
        // A deprecated parent keeps a link that still holds, so the link is
        // re-checked even when this comparison deprecated the parent. A
        // deprecated fork that was the successor has just been released.
        let transition = if fork_deprecated {
            if parent.superseded_by.as_deref() == Some(fork.id.as_str()) {
                SupersessionTransition::Withdraw
            } else {
                SupersessionTransition::None
            }
        } else {
            Self::settle_after_comparison_in(&mut transaction, &parent.id, &fork.id, timestamp)
                .await?
        };
        Self::record_event_in(&mut transaction, "comparison.recorded", Some(&fork.id), Some(&evaluator.id), serde_json::json!({
            "comparison_id": comparison.id, "parent_id": parent.id, "independent": comparison.independent, "authoritative": comparison.authoritative,
            "parent_authority": comparison.parent_authority, "complete": comparison.complete, "fork_safety": comparison.fork_safety,
            "protocol_digest": comparison.protocol_digest, "transition": transition.label(),
        })).await?;
        transaction.commit().await?;
        let mut fork = self
            .load_skill(&fork.id)
            .await?
            .ok_or(StoreError::NotFound)?;
        let mut parent = self
            .load_skill(&parent.id)
            .await?
            .ok_or(StoreError::NotFound)?;
        fork.summary = Some(self.aggregate(&fork.id).await?);
        parent.summary = Some(self.aggregate(&parent.id).await?);
        Ok((comparison, fork, parent, transition))
    }

    /// Final deprecation inside a write transaction; true when this call
    /// deprecated the skill.
    async fn deprecate_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        skill_id: &str,
        reason: &str,
        evidence: &Value,
        timestamp: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query("UPDATE skills SET status='deprecated', deprecation_reason=?, deprecated_at=?, updated_at=? WHERE id=? AND status IN ('candidate','verified')")
            .bind(reason).bind(timestamp).bind(timestamp).bind(skill_id)
            .execute(&mut **transaction).await?;
        if updated.rows_affected() != 1 {
            return Ok(false);
        }
        Self::record_event_in(transaction, "skill.deprecated", Some(skill_id), None, serde_json::json!({
            "decided_by": "comparison", "gate_version": SUPERSESSION_GATE_VERSION, "reason": reason, "evidence": evidence,
        })).await?;
        Ok(true)
    }

    /// Release the skill that `skill_id` succeeds, if any: after a
    /// deprecation of `skill_id`, its predecessor must not keep pointing at
    /// it.
    async fn release_predecessor_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        skill_id: &str,
        reason: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let predecessor: Option<String> = sqlx::query_scalar(
            "SELECT id FROM skills WHERE superseded_by=? AND typeof(id)='text' LIMIT 1",
        )
        .bind(skill_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some(parent_id) = predecessor {
            Self::release_successor_in(transaction, &parent_id, skill_id, reason, timestamp)
                .await?;
        }
        Ok(())
    }

    /// Withdraw the parent's link to `successor_id`: the link is cleared,
    /// the challenge the successor had addressed is open again, the gate is
    /// settled anew for the parent (so the oldest other verified fork that
    /// passes becomes the successor, or else the parent is active again), and
    /// every link below the released successor is re-checked, because its
    /// chain just became shorter.
    async fn release_successor_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        successor_id: &str,
        reason: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        if !Self::clear_link_in(transaction, parent_id, successor_id, reason, timestamp).await? {
            return Ok(());
        }
        Self::settle_supersession_in(transaction, parent_id, None, timestamp).await?;
        let mut current = successor_id.to_owned();
        for _ in 0..=MAX_LINEAGE_DEPTH {
            if Self::clear_unreadable_link_in(transaction, &current, timestamp).await? {
                Self::settle_supersession_in(transaction, &current, None, timestamp).await?;
                break;
            }
            let next: Option<String> =
                sqlx::query_scalar("SELECT superseded_by FROM skills WHERE id=?")
                    .bind(&current)
                    .fetch_optional(&mut **transaction)
                    .await?
                    .flatten();
            let Some(next) = next else {
                break;
            };
            if let Some(reason) = Self::link_failure_in(transaction, &current, &next).await? {
                Self::clear_link_in(transaction, &current, &next, reason, timestamp).await?;
                Self::settle_supersession_in(transaction, &current, None, timestamp).await?;
            }
            current = next;
        }
        Ok(())
    }

    /// Clear a link whose successor column holds something other than
    /// text, which no other query can name; true when one was cleared.
    async fn clear_unreadable_link_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let released = sqlx::query("UPDATE skills SET superseded_by=NULL, superseded_at=NULL, updated_at=? WHERE id=? AND superseded_by IS NOT NULL AND typeof(superseded_by)<>'text'")
            .bind(timestamp).bind(parent_id)
            .execute(&mut **transaction).await?;
        if released.rows_affected() == 0 {
            return Ok(false);
        }
        // The successor cannot be named, so every challenge this parent had
        // marked addressed is open again.
        sqlx::query("UPDATE challenges SET status='open', addressed_at=NULL, addressed_by_skill_id=NULL WHERE skill_id=? AND status='addressed'")
            .bind(parent_id)
            .execute(&mut **transaction).await?;
        Self::record_event_in(transaction, "skill.supersession_released", Some(parent_id), None, serde_json::json!({
            "successor_id": null, "reason": "unreadable", "gate_version": SUPERSESSION_GATE_VERSION,
        })).await?;
        Ok(true)
    }

    /// Clear one link, reopen the challenge its successor addressed and
    /// record the release; false when the link was no longer there.
    async fn clear_link_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        successor_id: &str,
        reason: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let released = sqlx::query("UPDATE skills SET superseded_by=NULL, superseded_at=NULL, updated_at=? WHERE id=? AND superseded_by=?")
            .bind(timestamp).bind(parent_id).bind(successor_id)
            .execute(&mut **transaction).await?;
        if released.rows_affected() != 1 {
            return Ok(false);
        }
        sqlx::query("UPDATE challenges SET status='open', addressed_at=NULL, addressed_by_skill_id=NULL WHERE skill_id=? AND addressed_by_skill_id=? AND status='addressed'")
            .bind(parent_id).bind(successor_id)
            .execute(&mut **transaction).await?;
        Self::record_event_in(transaction, "skill.supersession_released", Some(parent_id), None, serde_json::json!({
            "successor_id": successor_id, "reason": reason, "gate_version": SUPERSESSION_GATE_VERSION,
        })).await?;
        Ok(true)
    }

    /// Why the installed link from `parent_id` to `successor_id` no longer
    /// holds, or `None` when it holds (or is not there). A link holds while
    /// its successor exists, is readable, is a fork of the parent and is not
    /// deprecated, and its gate passes as an installed link.
    async fn link_failure_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        successor_id: &str,
    ) -> Result<Option<&'static str>, StoreError> {
        let load = |id: &str| sqlx::query("SELECT * FROM skills WHERE id=?").bind(id.to_owned());
        let Some(parent_row) = load(parent_id).fetch_optional(&mut **transaction).await? else {
            return Ok(None);
        };
        let Some(fork_row) = load(successor_id)
            .fetch_optional(&mut **transaction)
            .await?
        else {
            return Ok(Some("successor_missing"));
        };
        let (Ok(parent), Ok(fork)) = (row_to_skill(&parent_row), row_to_skill(&fork_row)) else {
            return Ok(Some("unreadable"));
        };
        if parent.superseded_by.as_deref() != Some(successor_id) {
            return Ok(None);
        }
        if fork.forked_by.is_none() || fork.parent_skill_id.as_deref() != Some(parent_id) {
            return Ok(Some("not_a_fork_of_the_parent"));
        }
        if fork.status == SkillStatus::Deprecated {
            return Ok(Some("successor_deprecated"));
        }
        match Self::gate_passes_in(transaction, &parent, &fork, true).await? {
            Some(true) => Ok(None),
            Some(false) => Ok(Some("gate_no_longer_holds")),
            None => Ok(Some("unreadable")),
        }
    }

    /// Whether the gate passes for `fork` over `parent` right now. `holding`
    /// checks a link that is already installed: the parent's successor is
    /// the fork itself, a successor the fork has gained since does not undo
    /// the link it holds, and neither does the parent's own later
    /// deprecation, because the successor's evidence stands on its own.
    /// `None` when the fork's comparisons cannot all be read, which fails
    /// the gate.
    async fn gate_passes_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent: &SkillArtifact,
        fork: &SkillArtifact,
        holding: bool,
    ) -> Result<Option<bool>, StoreError> {
        let Some(summaries) = Self::comparison_summaries_in(transaction, parent, fork).await?
        else {
            return Ok(None);
        };
        let family = fork
            .provenance
            .task_family
            .clone()
            .or_else(|| parent.provenance.task_family.clone());
        let parent_status = if holding {
            SkillStatus::Verified
        } else {
            parent.status
        };
        Ok(Some(
            supersession_gate(
                fork.status,
                parent_status,
                false,
                !holding && fork.superseded_by.is_some(),
                family.as_deref(),
                &summaries,
            ) == SupersessionTransition::Supersede,
        ))
    }

    /// Re-check every installed supersession link against the gate as it
    /// holds now and release each one that no longer holds, shallowest link
    /// first, so a parent's released link is gone before the links below it
    /// are judged against their own chain. Runs once per database after the
    /// migrations that change how links are judged (the epoch is kept in
    /// SQLite's `user_version`), so links recorded under older rules are
    /// judged by the current ones; later changes to authority re-check only
    /// the links they touch. Idempotent; returns how many links were
    /// released.
    pub async fn revalidate_supersessions(&self) -> Result<usize, StoreError> {
        Ok(self.revalidate(true).await?.unwrap_or_default())
    }

    /// The full re-check in one write transaction; unless `force`, only when
    /// the epoch read inside that transaction is older than the current one,
    /// so two processes starting together run it once. `None` when it did
    /// not run.
    async fn revalidate(&self, force: bool) -> Result<Option<usize>, StoreError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if !force {
            let epoch: i64 = sqlx::query_scalar("PRAGMA user_version")
                .fetch_one(&mut *transaction)
                .await?;
            if epoch >= REVALIDATION_EPOCH {
                transaction.commit().await?;
                return Ok(None);
            }
        }
        let timestamp = now();
        // Links no query can name (a non-text value) are released first.
        let unnameable: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM skills WHERE superseded_by IS NOT NULL AND typeof(superseded_by)<>'text' AND typeof(id)='text'",
        )
        .fetch_all(&mut *transaction)
        .await?;
        let mut unreadable = sqlx::query("UPDATE skills SET superseded_by=NULL, superseded_at=NULL, updated_at=? WHERE superseded_by IS NOT NULL AND typeof(id)<>'text'")
            .bind(timestamp)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
        // A link whose successor no query can name is released like any
        // other: the challenges it addressed reopen and the parent settles
        // anew.
        for parent_id in &unnameable {
            if Self::clear_unreadable_link_in(&mut transaction, parent_id, timestamp).await? {
                Self::settle_supersession_in(&mut transaction, parent_id, None, timestamp).await?;
                unreadable += 1;
            }
        }
        if unreadable > 0 {
            Self::record_event_in(&mut transaction, "skill.supersession_released", None, None, serde_json::json!({
                "successor_id": null, "reason": "unreadable", "released": unreadable, "gate_version": SUPERSESSION_GATE_VERSION,
            })).await?;
        }
        let links: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, superseded_by FROM skills WHERE superseded_by IS NOT NULL ORDER BY lineage_depth ASC, created_at ASC, id ASC",
        )
        .fetch_all(&mut *transaction)
        .await?;
        let released = Self::recheck_links_in(&mut transaction, links, timestamp).await?
            + usize::try_from(unreadable).unwrap_or(usize::MAX);
        sqlx::query(&format!("PRAGMA user_version = {REVALIDATION_EPOCH}"))
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(released))
    }

    /// Run the full re-check when this database has not had it for the
    /// current epoch. A failure (a busy database, for instance) leaves the
    /// epoch unrecorded, so the next start tries again.
    async fn revalidate_if_needed(&self) -> Result<(), StoreError> {
        let epoch: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await?;
        if epoch < REVALIDATION_EPOCH
            && let Some(released) = self.revalidate(false).await?
        {
            tracing::info!(released, "supersession links re-checked");
        }
        Ok(())
    }

    /// Re-check the given links in order and release each that no longer
    /// holds.
    async fn recheck_links_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        links: Vec<(String, String)>,
        timestamp: DateTime<Utc>,
    ) -> Result<usize, StoreError> {
        let mut released = 0;
        for (parent_id, successor_id) in links {
            if let Some(reason) =
                Self::link_failure_in(transaction, &parent_id, &successor_id).await?
            {
                Self::release_successor_in(
                    transaction,
                    &parent_id,
                    &successor_id,
                    reason,
                    timestamp,
                )
                .await?;
                released += 1;
            }
        }
        Ok(released)
    }

    /// Re-check the links whose successor has a comparison by `principal_id`,
    /// after that principal's standing changed: a revoked or disabled
    /// evaluator's links fall, and a restored one's later regression counts
    /// again.
    async fn recheck_principal_links_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        principal_id: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<usize, StoreError> {
        let links: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT s.id, s.superseded_by FROM skills s JOIN comparisons c ON c.fork_id = s.superseded_by WHERE c.principal_id=? AND typeof(s.id)='text' AND typeof(s.superseded_by)='text' ORDER BY s.lineage_depth ASC, s.id ASC",
        )
        .bind(principal_id)
        .fetch_all(&mut **transaction)
        .await?;
        Self::recheck_links_in(transaction, links, timestamp).await
    }

    /// After a comparison: withdraw the parent's link to the fork when the
    /// gate no longer holds for it, or else settle the gate for the fork.
    /// Returns what happened to the link between them.
    async fn settle_after_comparison_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        fork_id: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<SupersessionTransition, StoreError> {
        let load = |id: &str| sqlx::query("SELECT * FROM skills WHERE id=?").bind(id.to_owned());
        let Some(parent_row) = load(parent_id).fetch_optional(&mut **transaction).await? else {
            return Ok(SupersessionTransition::None);
        };
        let parent = row_to_skill(&parent_row)?;
        if parent.superseded_by.as_deref() == Some(fork_id) {
            let Some(reason) = Self::link_failure_in(transaction, parent_id, fork_id).await? else {
                return Ok(SupersessionTransition::None);
            };
            Self::release_successor_in(transaction, parent_id, fork_id, reason, timestamp).await?;
            return Ok(SupersessionTransition::Withdraw);
        }
        Ok(
            match Self::settle_supersession_in(transaction, parent_id, Some(fork_id), timestamp)
                .await?
            {
                Some(successor) if successor == fork_id => SupersessionTransition::Supersede,
                _ => SupersessionTransition::None,
            },
        )
    }

    /// Settle the supersession gate for a parent inside a write transaction:
    /// for one fork (after its comparison or verification), or for every
    /// verified fork oldest first (after the parent was released). Returns
    /// the new successor when one was installed. The parent must be
    /// verified, and any successor it has must be deprecated; the
    /// compare-and-set repeats both conditions.
    async fn settle_supersession_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        parent_id: &str,
        only_fork: Option<&str>,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<String>, StoreError> {
        let Some(parent_row) = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(parent_id)
            .fetch_optional(&mut **transaction)
            .await?
        else {
            return Ok(None);
        };
        let Ok(parent) = row_to_skill(&parent_row) else {
            return Ok(None);
        };
        if parent.status != SkillStatus::Verified {
            return Ok(None);
        }
        if let Some(successor) = &parent.superseded_by {
            let deprecated: Option<i64> =
                sqlx::query_scalar("SELECT status='deprecated' FROM skills WHERE id=?")
                    .bind(successor)
                    .fetch_optional(&mut **transaction)
                    .await?;
            if deprecated != Some(1) {
                return Ok(None);
            }
        }
        let rows = match only_fork {
            Some(fork_id) => sqlx::query("SELECT * FROM skills WHERE id=? AND parent_skill_id=? AND forked_by_id IS NOT NULL")
                .bind(fork_id).bind(parent_id)
                .fetch_all(&mut **transaction).await?,
            None => sqlx::query("SELECT * FROM skills WHERE parent_skill_id=? AND forked_by_id IS NOT NULL AND status='verified' ORDER BY created_at ASC, id ASC")
                .bind(parent_id)
                .fetch_all(&mut **transaction).await?,
        };
        for row in &rows {
            let Ok(fork) = row_to_skill(row) else {
                continue;
            };
            if Some(&fork.id) == parent.superseded_by.as_ref() {
                continue;
            }
            if Self::gate_passes_in(transaction, &parent, &fork, false).await? != Some(true) {
                continue;
            }
            let updated = sqlx::query("UPDATE skills SET superseded_by=?, superseded_at=?, updated_at=? WHERE id=? AND status='verified' AND (superseded_by IS NULL OR superseded_by IN (SELECT id FROM skills WHERE status='deprecated'))")
                .bind(&fork.id).bind(timestamp).bind(timestamp).bind(parent_id)
                .execute(&mut **transaction).await?;
            if updated.rows_affected() != 1 {
                return Ok(None);
            }
            if let Some(challenge_id) = &fork.responding_to_challenge_id {
                sqlx::query("UPDATE challenges SET status='addressed', addressed_at=?, addressed_by_skill_id=? WHERE id=? AND status='open'")
                    .bind(timestamp).bind(&fork.id).bind(challenge_id)
                    .execute(&mut **transaction).await?;
            }
            Self::record_event_in(transaction, "skill.superseded", Some(parent_id), None, serde_json::json!({
                "successor_id": fork.id, "previous_successor_id": parent.superseded_by, "decided_by": "gate",
                "gate_version": SUPERSESSION_GATE_VERSION, "responding_to_challenge_id": fork.responding_to_challenge_id,
            })).await?;
            return Ok(Some(fork.id));
        }
        Ok(None)
    }

    // -- forum: challenges --

    /// A receipt by id: `None` when there is none, `Some(Err(_))` when its
    /// row cannot be read.
    async fn receipt_by_id(
        &self,
        id: &str,
    ) -> Result<Option<Result<SkillReceiptItem, StoreError>>, StoreError> {
        let row = sqlx::query(RECEIPT_BY_ID)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.as_ref().map(row_to_receipt))
    }

    /// Record a challenge against the exact current version of a skill the
    /// challenger can see. The challenger is the authenticated principal;
    /// the text is re-sanitized here; a referenced receipt must already be
    /// recorded on the same skill. The same claim by the same principal is
    /// one challenge. The challenger is judged as it stands inside the write
    /// transaction. Nothing here changes the skill's status.
    pub async fn create_challenge(
        &self,
        challenger: &Principal,
        skill_id: &str,
        submission: &ChallengeSubmission,
    ) -> Result<(ChallengeItem, bool), StoreError> {
        let submission = validate_challenge(submission).map_err(StoreError::Invalid)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &challenger.id).await?;
        let challenger = &current;
        let viewer = Viewer {
            principal: Some(challenger.clone()),
        };
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(skill_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let skill = row_to_skill(&row)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        let evidence = match &submission.evidence_receipt_id {
            Some(receipt_id) => {
                let row = sqlx::query(RECEIPT_BY_ID)
                    .bind(receipt_id)
                    .fetch_optional(&mut *transaction)
                    .await?;
                let receipt = row
                    .as_ref()
                    .map(row_to_receipt)
                    .transpose()?
                    .filter(|receipt| receipt.skill_id == skill.id)
                    .ok_or_else(|| {
                        StoreError::Invalid(
                            "evidence_receipt_id must name a receipt recorded on this skill".into(),
                        )
                    })?;
                Some(challenge_evidence(&receipt))
            }
            None => None,
        };
        let digest = challenge_digest(&submission);
        let existing = sqlx::query(
            "SELECT * FROM challenges WHERE skill_id=? AND challenger_id=? AND claim_digest=?",
        )
        .bind(&skill.id)
        .bind(&challenger.id)
        .bind(&digest)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(row) = existing {
            let item = row_to_challenge(&row, evidence)?;
            transaction.commit().await?;
            return Ok((item, false));
        }
        let on_skill: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM challenges WHERE skill_id=?")
            .bind(&skill.id)
            .fetch_one(&mut *transaction)
            .await?;
        if on_skill >= i64::try_from(MAX_CHALLENGES_PER_SKILL).unwrap_or(i64::MAX) {
            return Err(StoreError::Conflict(format!(
                "a skill may collect at most {MAX_CHALLENGES_PER_SKILL} challenges"
            )));
        }
        let by_challenger: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM challenges WHERE skill_id=? AND challenger_id=?",
        )
        .bind(&skill.id)
        .bind(&challenger.id)
        .fetch_one(&mut *transaction)
        .await?;
        if by_challenger >= i64::try_from(MAX_CHALLENGES_PER_PRINCIPAL).unwrap_or(i64::MAX) {
            return Err(StoreError::Conflict(format!(
                "one principal may file at most {MAX_CHALLENGES_PER_PRINCIPAL} challenges against one skill"
            )));
        }
        let timestamp = now();
        let item = ChallengeItem {
            id: new_id("challenge"),
            skill_id: skill.id.clone(),
            content_digest: skill.content_digest.clone(),
            skill_version: skill.version,
            challenger: challenger.as_skill_principal(),
            kind: submission.kind,
            claim: submission.claim.clone(),
            applicability: submission.applicability.clone(),
            evidence_backed: evidence.is_some(),
            evidence,
            status: ChallengeStatus::Open,
            created_at: timestamp,
            addressed_at: None,
            addressed_by_skill_id: None,
        };
        sqlx::query("INSERT INTO challenges (id, skill_id, content_digest, skill_version, challenger_id, challenger_display, kind, claim, applicability, evidence_receipt_id, claim_digest, status, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,'open',?)")
            .bind(&item.id).bind(&item.skill_id).bind(&item.content_digest).bind(i64::from(item.skill_version))
            .bind(&challenger.id).bind(&challenger.display_name).bind(item.kind.as_str()).bind(&item.claim)
            .bind(&item.applicability).bind(&submission.evidence_receipt_id).bind(&digest).bind(timestamp)
            .execute(&mut *transaction).await?;
        Self::record_event_in(&mut transaction, "challenge.created", Some(&skill.id), Some(&challenger.id), serde_json::json!({
            "challenge_id": item.id, "kind": item.kind, "content_digest": item.content_digest, "skill_version": item.skill_version,
            "evidence_receipt_id": submission.evidence_receipt_id, "evidence_backed": item.evidence_backed,
        })).await?;
        transaction.commit().await?;
        Ok((item, true))
    }

    /// One page of the challenges recorded against a skill the viewer can
    /// see, newest first, filtered before the page is cut, with the evidence
    /// receipt as the backend records it today.
    pub async fn list_challenges(
        &self,
        viewer: &Viewer,
        skill_id: &str,
        filter: ChallengeFilter,
        page: &ForumPage,
    ) -> Result<Vec<ChallengeItem>, StoreError> {
        let skill = self
            .load_skill(skill_id)
            .await?
            .ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        if let Some(before) = &page.before {
            let known: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM challenges WHERE id=? AND skill_id=?")
                    .bind(before)
                    .bind(skill_id)
                    .fetch_one(&self.pool)
                    .await?;
            if known != 1 {
                return Err(StoreError::Invalid(
                    "before must name a challenge of this skill".into(),
                ));
            }
        }
        let condition = match filter {
            ChallengeFilter::All => "",
            ChallengeFilter::Open => " AND status='open'",
            ChallengeFilter::EvidenceBacked => " AND evidence_receipt_id IS NOT NULL",
        };
        let cursor = if page.before.is_some() {
            " AND (created_at, id) < (SELECT created_at, id FROM challenges WHERE id=?)"
        } else {
            ""
        };
        let sql = format!(
            "SELECT * FROM challenges WHERE skill_id=?{condition}{cursor} ORDER BY created_at DESC, id DESC LIMIT ?"
        );
        let mut query = sqlx::query(&sql).bind(skill_id);
        if let Some(before) = &page.before {
            query = query.bind(before);
        }
        let rows = query
            .bind(i64::from(list_limit(page.limit)))
            .fetch_all(&self.pool)
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            // A challenge whose evidence cannot be read is unreadable too:
            // it is skipped rather than shown without its evidence.
            let Ok(evidence_id) = row.try_get::<Option<String>, _>("evidence_receipt_id") else {
                continue;
            };
            let evidence = match evidence_id {
                Some(id) => match self.receipt_by_id(&id).await? {
                    Some(Ok(receipt)) => Some(challenge_evidence(&receipt)),
                    Some(Err(_)) => continue,
                    None => None,
                },
                None => None,
            };
            if let Ok(item) = row_to_challenge(row, evidence) {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Explicit, final deprecation by a member of the skill's team, judged
    /// on the principal as it stands inside the write transaction.
    pub async fn deprecate(
        &self,
        principal: &Principal,
        skill_id: &str,
        reason: &str,
    ) -> Result<SkillArtifact, StoreError> {
        let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
        if reason.is_empty()
            || reason.chars().count() > MAX_SKILL_REASON_CHARS
            || reason.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid(format!(
                "a deprecation reason must contain 1 to {MAX_SKILL_REASON_CHARS} printable characters"
            )));
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = Self::current_principal_in(&mut transaction, &principal.id).await?;
        let principal = &current;
        let row = sqlx::query("SELECT * FROM skills WHERE id=?")
            .bind(skill_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StoreError::NotFound)?;
        let skill = row_to_skill(&row)?;
        let viewer = Viewer {
            principal: Some(principal.clone()),
        };
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        if !viewer.in_team(
            &skill.shared_scope.organization_id,
            &skill.shared_scope.team_id,
        ) {
            return Err(StoreError::Forbidden);
        }
        let timestamp = now();
        let updated = sqlx::query("UPDATE skills SET status='deprecated', deprecation_reason=?, deprecated_at=?, updated_at=? WHERE id=? AND status IN ('candidate','verified')")
            .bind(&reason).bind(timestamp).bind(timestamp).bind(skill_id)
            .execute(&mut *transaction).await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "skill {skill_id} is already {}",
                skill.status.as_str()
            )));
        }
        Self::record_event_in(&mut transaction, "skill.deprecated", Some(skill_id), Some(&principal.id), serde_json::json!({
            "decided_by": principal.id, "reason": reason, "safety": reason == SAFETY_DEPRECATION_REASON,
        })).await?;
        Self::release_predecessor_in(
            &mut transaction,
            skill_id,
            "successor_deprecated",
            timestamp,
        )
        .await?;
        transaction.commit().await?;
        self.get_skill(&viewer, skill_id).await
    }
}
