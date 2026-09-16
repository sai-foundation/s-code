//! SQLite persistence for the remote registry. Every status transition is
//! decided by the shared domain gate inside the same `BEGIN IMMEDIATE`
//! transaction that records the receipt, exactly like the local shop, so
//! concurrent receipts cannot verify twice, lose a receipt, count one
//! principal twice, resurrect a deprecated skill or split the receipt from
//! its transition.
use crate::auth::{generate_token, token_hash};
use chrono::{DateTime, Utc};
use s_code_skill_shop::{
    MAX_SKILL_REASON_CHARS, ReceiptSummary, SAFETY_DEPRECATION_REASON, SKILL_GATE_VERSION,
    SharedScope, SkillAggregate, SkillArtifact, SkillPrincipal, SkillProvenance, SkillPublication,
    SkillReceiptItem, SkillReceiptSubmission, SkillSafety, SkillStatus, SkillTransition,
    SkillVisibility, aggregate_receipts, receipt_verdict, skill_gate, validate_publication,
    validate_receipt,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{path::Path, str::FromStr, time::Duration};

/// Receipts joined with their evaluator's current state, so a disabled
/// principal's receipts stop counting without being rewritten.
const RECEIPTS_OLDEST_FIRST: &str = "SELECT r.*, p.disabled AS evaluator_disabled FROM receipts r JOIN principals p ON p.id = r.principal_id WHERE r.skill_id=? ORDER BY r.created_at ASC, r.id ASC";
const RECEIPTS_NEWEST_FIRST: &str = "SELECT r.*, p.disabled AS evaluator_disabled FROM receipts r JOIN principals p ON p.id = r.principal_id WHERE r.skill_id=? ORDER BY r.created_at DESC, r.id DESC";

pub const MAX_LIST_LIMIT: u32 = 100;
pub const DEFAULT_LIST_LIMIT: u32 = 50;
const MAX_RESULT_BYTES: usize = 64 * 1024;

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
    let authoritative: i64 = row.try_get("authoritative")?;
    // A receipt stops counting once its evaluator principal is disabled;
    // the receipt itself stays on record.
    let evaluator_disabled: i64 = row.try_get("evaluator_disabled").unwrap_or(0);
    Ok(SkillReceiptItem {
        id: row.try_get("id")?,
        skill_id: row.try_get("skill_id")?,
        evaluator: SkillPrincipal {
            id: row.try_get("principal_id")?,
            display_name: Some(row.try_get("principal_display")?),
        },
        evaluator_program: serde_json::from_str(&evaluator).unwrap_or(Value::Null),
        independent: independent != 0,
        authoritative: authoritative != 0 && evaluator_disabled == 0,
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

impl RegistryStore {
    pub async fn open(data_dir: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(data_dir)
            .map_err(|error| StoreError::Storage(error.to_string()))?;
        let path = data_dir.join("registry.db");
        Self::connect(&format!("sqlite://{}", path.display()), true).await
    }

    pub async fn in_memory() -> Result<Self, StoreError> {
        Self::connect("sqlite::memory:", false).await
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
        Ok(Self { pool })
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

    /// Grant or revoke the authorized-evaluator capability. Existing
    /// receipts keep the authority they were recorded with; the change
    /// applies to receipts submitted from now on.
    pub async fn set_authorized_evaluator(
        &self,
        id: &str,
        authorized: bool,
    ) -> Result<Principal, StoreError> {
        let updated = sqlx::query("UPDATE principals SET authorized_evaluator=? WHERE id=?")
            .bind(i64::from(authorized))
            .bind(id)
            .execute(&self.pool)
            .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::NotFound);
        }
        self.record_event(
            if authorized {
                "principal.evaluator_authorized"
            } else {
                "principal.evaluator_revoked"
            },
            None,
            Some(id),
            serde_json::json!({}),
        )
        .await?;
        self.get_principal(id).await?.ok_or(StoreError::NotFound)
    }

    pub async fn disable_principal(&self, id: &str) -> Result<Principal, StoreError> {
        let updated = sqlx::query("UPDATE principals SET disabled=1 WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::NotFound);
        }
        self.record_event("principal.disabled", None, Some(id), serde_json::json!({}))
            .await?;
        self.get_principal(id).await?.ok_or(StoreError::NotFound)
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

    /// Publish for the authenticated principal. The payload is validated
    /// independently here; the same team publishing the same content again
    /// receives the same skill (content addressing), so retries never create
    /// duplicates.
    pub async fn publish(
        &self,
        publisher: &Principal,
        publication: &SkillPublication,
    ) -> Result<(SkillArtifact, bool), StoreError> {
        let publication = validate_publication(publication).map_err(StoreError::Invalid)?;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
            summary: None,
        };
        sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, deprecation_reason, parent_skill_id, version, provenance_json, created_at, updated_at, verified_at, deprecated_at) VALUES (?,?,?,?,?,?,?,?,?,?,'candidate',NULL,?,?,?,?,?,NULL,NULL)")
            .bind(&skill.id).bind(&skill.shared_scope.organization_id).bind(&skill.shared_scope.team_id)
            .bind(&publisher.id).bind(&publisher.display_name).bind(skill.visibility.as_str())
            .bind(&skill.lesson).bind(&skill.applicability).bind(&skill.content_digest).bind(i64::from(skill.sanitization_version))
            .bind(&skill.parent_skill_id).bind(i64::from(skill.version))
            .bind(serde_json::to_string(&skill.provenance).unwrap_or_else(|_| "{}".into()))
            .bind(timestamp).bind(timestamp)
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

    /// A skill the viewer may see, with its catalog summary.
    pub async fn get_skill(&self, viewer: &Viewer, id: &str) -> Result<SkillArtifact, StoreError> {
        let mut skill = self.load_skill(id).await?.ok_or(StoreError::NotFound)?;
        if !viewer.can_see(&skill) {
            return Err(StoreError::NotFound);
        }
        skill.summary = Some(self.aggregate(&skill.id).await?);
        Ok(skill)
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
            let skill = row_to_skill(row)?;
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
        Ok(page.into_iter().skip(offset).take(limit).collect())
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
        rows.iter().map(row_to_receipt).collect()
    }

    async fn summaries_in(
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        skill_id: &str,
    ) -> Result<Vec<ReceiptSummary>, StoreError> {
        let rows = sqlx::query(RECEIPTS_OLDEST_FIRST)
            .bind(skill_id)
            .fetch_all(&mut **transaction)
            .await?;
        rows.iter()
            .map(|row| row_to_receipt(row).map(|item| ReceiptSummary::from(&item)))
            .collect()
    }

    pub async fn aggregate(&self, skill_id: &str) -> Result<SkillAggregate, StoreError> {
        let rows = sqlx::query(RECEIPTS_OLDEST_FIRST)
            .bind(skill_id)
            .fetch_all(&self.pool)
            .await?;
        let summaries = rows
            .iter()
            .map(|row| row_to_receipt(row).map(|item| ReceiptSummary::from(&item)))
            .collect::<Result<Vec<_>, _>>()?;
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
        let viewer = Viewer {
            principal: Some(evaluator.clone()),
        };
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
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
        let summaries = Self::summaries_in(&mut transaction, &skill.id).await?;
        let requested = skill_gate(skill.status, &summaries);
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
        transaction.commit().await?;
        skill.summary = Some(self.aggregate(&skill.id).await?);
        Ok(RecordedReceipt {
            receipt,
            skill,
            transition,
        })
    }

    /// Explicit, final deprecation by a member of the skill's team.
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
        let skill = self
            .load_skill(skill_id)
            .await?
            .ok_or(StoreError::NotFound)?;
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
            .execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "skill {skill_id} is already {}",
                skill.status.as_str()
            )));
        }
        self.record_event("skill.deprecated", Some(skill_id), Some(&principal.id), serde_json::json!({
            "decided_by": principal.id, "reason": reason, "safety": reason == SAFETY_DEPRECATION_REASON,
        })).await?;
        self.get_skill(&viewer, skill_id).await
    }
}
