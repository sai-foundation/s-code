use super::{StorageError, Store};
use chrono::{DateTime, Utc};
use s_code_protocol::{Id, Scope};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::RwLock;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileProtectionRule {
    pub id: Id,
    pub path: String,
    pub canonical_path: Option<String>,
    pub device: Option<u64>,
    pub inode: Option<u64>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileProtectionPolicy {
    pub revision: u64,
    pub changed_at: Option<DateTime<Utc>>,
    pub rules: Vec<FileProtectionRule>,
}

type ProtectionOwner = (String, String, String);
#[derive(Clone, Default)]
pub(crate) struct FileProtectionGates(Arc<Mutex<HashMap<ProtectionOwner, Arc<RwLock<()>>>>>);

fn owner(scope: &Scope) -> ProtectionOwner {
    (
        scope.organization_id.0.clone(),
        scope.team_id.0.clone(),
        scope.actor_id.0.clone(),
    )
}

// SensitiveCodec's common AAD contains organization/team but not actor. Bind
// the complete owner in its record id to prevent ciphertext swaps across actors.
fn record_id(scope: &Scope) -> Id {
    Id(serde_json::to_string(&owner(scope)).expect("owner tuple is serializable"))
}

impl Store {
    /// Clones of this Store share this gate. Hold a read guard across policy
    /// inspection and model dispatch; replacement holds a write guard to commit.
    pub fn protection_gate(&self, scope: &Scope) -> Arc<RwLock<()>> {
        self.protection_gates
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(owner(scope))
            .or_default()
            .clone()
    }

    /// Does not acquire the gate: callers may already hold a dispatch read guard.
    pub async fn file_protection(
        &self,
        scope: &Scope,
    ) -> Result<FileProtectionPolicy, StorageError> {
        let row = sqlx::query(
            "SELECT revision,changed_at,rules_json FROM file_protection
             WHERE organization_id=? AND team_id=? AND actor_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(FileProtectionPolicy::default());
        };
        let revision = u64::try_from(row.try_get::<i64, _>("revision")?)
            .map_err(|_| StorageError::InvalidData("invalid file protection revision".into()))?;
        let rules_json: String = row.try_get("rules_json")?;
        let plaintext = self.sensitive.open_text(
            scope,
            "file_protection",
            &record_id(scope),
            "rules_json",
            &rules_json,
        )?;
        let rules = serde_json::from_str(&plaintext)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        Ok(FileProtectionPolicy {
            revision,
            changed_at: Some(row.try_get("changed_at")?),
            rules,
        })
    }

    pub async fn replace_file_protection(
        &self,
        scope: &Scope,
        expected_revision: u64,
        rules: Vec<FileProtectionRule>,
    ) -> Result<FileProtectionPolicy, StorageError> {
        if rules.len() > 256 {
            return Err(StorageError::InvalidData(
                "at most 256 file protection rules are allowed".into(),
            ));
        }
        let gate = self.protection_gate(scope);
        let _activation_guard = gate.write().await;
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query(
            "SELECT revision,changed_at FROM file_protection
             WHERE organization_id=? AND team_id=? AND actor_id=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_optional(&mut *transaction)
        .await?;
        let previous_revision = match &existing {
            Some(row) => row.try_get::<i64, _>("revision")?,
            None => 0,
        };
        if u64::try_from(previous_revision).ok() != Some(expected_revision) {
            return Err(StorageError::InvalidState(
                "file protection revision conflict".into(),
            ));
        }
        let revision = previous_revision.checked_add(1).ok_or_else(|| {
            StorageError::InvalidState("file protection revision exhausted".into())
        })?;
        let mut changed_at = Utc::now();
        if let Some(row) = existing {
            let previous: DateTime<Utc> = row.try_get("changed_at")?;
            let next = previous
                .checked_add_signed(chrono::Duration::nanoseconds(1))
                .ok_or_else(|| {
                    StorageError::InvalidState("file protection cutoff exhausted".into())
                })?;
            changed_at = changed_at.max(next);
        }
        // Wall time can move backwards before the first activation too. Every
        // already-persisted source must fall behind this barrier, including
        // queued work that has not produced a Turn yet. Team goal/task text can
        // be dispatched by this actor even when another actor authored it.
        let latest: Option<DateTime<Utc>> = sqlx::query_scalar(
            "SELECT MAX(provenance_at) FROM (
                SELECT started_at AS provenance_at FROM turns
                    WHERE organization_id=?1 AND team_id=?2 AND actor_id=?3
                UNION ALL SELECT created_at FROM turn_inputs
                    WHERE organization_id=?1 AND team_id=?2 AND actor_id=?3
                UNION ALL SELECT created_at FROM durable_tasks
                    WHERE organization_id=?1 AND team_id=?2 AND actor_id=?3
                UNION ALL SELECT created_at FROM session_goals
                    WHERE organization_id=?1 AND team_id=?2 AND actor_id=?3
                UNION ALL SELECT created_at FROM team_goal_runs
                    WHERE organization_id=?1 AND team_id=?2 AND actor_id=?3
                UNION ALL SELECT created_at FROM team_goals
                    WHERE organization_id=?1 AND team_id=?2
                UNION ALL SELECT created_at FROM team_tasks
                    WHERE organization_id=?1 AND team_id=?2
            )",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .fetch_one(&mut *transaction)
        .await?;
        if let Some(latest) = latest {
            let next = latest
                .checked_add_signed(chrono::Duration::nanoseconds(1))
                .ok_or_else(|| {
                    StorageError::InvalidState("file protection cutoff exhausted".into())
                })?;
            changed_at = changed_at.max(next);
        }
        let plaintext = serde_json::to_string(&rules)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let sealed = self.sensitive.seal_text(
            scope,
            "file_protection",
            &record_id(scope),
            "rules_json",
            &plaintext,
        )?;
        sqlx::query(
            "INSERT INTO file_protection (organization_id,team_id,actor_id,revision,changed_at,rules_json)
             VALUES (?,?,?,?,?,?) ON CONFLICT(organization_id,team_id,actor_id)
             DO UPDATE SET revision=excluded.revision,changed_at=excluded.changed_at,rules_json=excluded.rules_json",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(revision)
        .bind(changed_at)
        .bind(sealed)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(FileProtectionPolicy {
            revision: revision as u64,
            changed_at: Some(changed_at),
            rules,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(actor: &str) -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id(actor.into()),
            goal_id: None,
            task_id: None,
        }
    }

    fn rule() -> FileProtectionRule {
        FileProtectionRule {
            id: Id("protected-one".into()),
            path: "/private/payroll.csv".into(),
            canonical_path: Some("/private/payroll.csv".into()),
            device: Some(1),
            inode: Some(2),
            kind: "file".into(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn file_protection_encrypts_isolates_accounts_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}",
            directory.path().join("state/policy.sqlite").display()
        );
        let store = Store::connect_encrypted(&url, "protection", &[7; 32])
            .await
            .unwrap();
        let owner = scope("alice");
        let policy = store
            .replace_file_protection(&owner, 0, vec![rule()])
            .await
            .unwrap();
        assert_eq!(policy.revision, 1);
        let encoded: String = sqlx::query_scalar("SELECT rules_json FROM file_protection")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(encoded.starts_with("enc:v1:"));
        assert!(!encoded.contains("payroll"));
        assert!(!encoded.contains("protected-one"));
        for other in [
            scope("bob"),
            Scope {
                team_id: Id("other".into()),
                ..owner.clone()
            },
            Scope {
                organization_id: Id("other".into()),
                ..owner.clone()
            },
        ] {
            assert_eq!(
                store.file_protection(&other).await.unwrap(),
                FileProtectionPolicy::default()
            );
        }
        let task_scope = Scope {
            goal_id: Some(Id("goal".into())),
            task_id: Some(Id("task".into())),
            ..owner.clone()
        };
        assert_eq!(store.file_protection(&task_scope).await.unwrap(), policy);
        assert!(Arc::ptr_eq(
            &store.protection_gate(&owner),
            &store.clone().protection_gate(&task_scope)
        ));
        assert!(!Arc::ptr_eq(
            &store.protection_gate(&owner),
            &store.protection_gate(&scope("bob"))
        ));
        store.pool.close().await;
        let reopened = Store::connect_encrypted(&url, "protection", &[7; 32])
            .await
            .unwrap();
        assert_eq!(reopened.file_protection(&owner).await.unwrap(), policy);
        // Actor identity is authenticated even though common codec AAD only
        // carries the organization/team explicitly.
        sqlx::query("INSERT INTO file_protection SELECT organization_id,team_id,'bob',revision,changed_at,rules_json FROM file_protection WHERE actor_id='alice'")
            .execute(&reopened.pool).await.unwrap();
        assert!(matches!(
            reopened.file_protection(&scope("bob")).await,
            Err(StorageError::Encryption(_))
        ));
    }

    #[tokio::test]
    async fn file_protection_cas_and_empty_rules_preserve_monotonic_barrier() {
        let store = Store::in_memory().await.unwrap();
        let owner = scope("alice");
        assert_eq!(
            store.file_protection(&owner).await.unwrap(),
            FileProtectionPolicy::default()
        );
        let first = store
            .replace_file_protection(&owner, 0, vec![rule()])
            .await
            .unwrap();
        assert!(matches!(
            store.replace_file_protection(&owner, 0, vec![]).await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(store.file_protection(&owner).await.unwrap(), first);
        // A clock moving backwards must not reopen older transcript context.
        let future = Utc::now() + chrono::Duration::days(1);
        sqlx::query("UPDATE file_protection SET changed_at=?")
            .bind(future)
            .execute(&store.pool)
            .await
            .unwrap();
        let empty = store
            .replace_file_protection(&owner, 1, vec![])
            .await
            .unwrap();
        assert_eq!(empty.revision, 2);
        assert!(empty.rules.is_empty());
        assert!(empty.changed_at.unwrap() > future);
        assert_eq!(store.file_protection(&owner).await.unwrap(), empty);
        let empty_again = store
            .replace_file_protection(&owner, 2, vec![])
            .await
            .unwrap();
        assert!(empty_again.changed_at > empty.changed_at);
        assert!(matches!(
            store
                .replace_file_protection(&owner, 3, vec![rule(); 257])
                .await,
            Err(StorageError::InvalidData(_))
        ));
        assert_eq!(store.file_protection(&owner).await.unwrap(), empty_again);
    }

    #[tokio::test]
    async fn file_protection_dispatch_gate_blocks_activation_until_readers_release() {
        let store = Store::in_memory().await.unwrap();
        let owner = scope("alice");
        let gate = store.protection_gate(&owner);
        let dispatch = gate.read().await;
        let writer_store = store.clone();
        let writer_owner = owner.clone();
        let mut replacement = tokio::spawn(async move {
            writer_store
                .replace_file_protection(&writer_owner, 0, vec![rule()])
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut replacement)
                .await
                .is_err()
        );
        // Getter remains usable with a held read guard and a queued writer.
        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                store.file_protection(&owner)
            )
            .await
            .unwrap()
            .unwrap()
            .revision,
            0
        );
        drop(dispatch);
        let updated = tokio::time::timeout(std::time::Duration::from_secs(2), replacement)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(updated.revision, 1);
        let _dispatch = gate.read().await;
        assert_eq!(store.file_protection(&owner).await.unwrap(), updated);
    }

    #[tokio::test]
    async fn file_protection_cas_serializes_independent_connections() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}",
            directory.path().join("state/policy.sqlite").display()
        );
        let first = Store::connect(&url).await.unwrap();
        let second = Store::connect(&url).await.unwrap();
        let owner = scope("alice");
        let (a, b) = tokio::join!(
            first.replace_file_protection(&owner, 0, vec![rule()]),
            second.replace_file_protection(&owner, 0, vec![]),
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(
            a.err().or_else(|| b.err()),
            Some(StorageError::InvalidState(_))
        ));
        assert_eq!(first.file_protection(&owner).await.unwrap().revision, 1);
    }
    #[tokio::test]
    async fn file_protection_first_activation_covers_future_persisted_provenance() {
        let store = Store::in_memory().await.unwrap();
        let owner = scope("alice");
        let session = store
            .create_session(s_code_protocol::CreateSession {
                scope: owner.clone(),
                mode: s_code_protocol::SessionMode::Chat,
                workspace_uri: String::new(),
                title: "Clock rollback".into(),
                model: "fixture".into(),
            })
            .await
            .unwrap();
        let old = store.create_turn(&owner, &session.id).await.unwrap();
        let future = Utc::now() + chrono::Duration::days(2);
        sqlx::query("UPDATE turns SET started_at=? WHERE id=?")
            .bind(future)
            .bind(&old.id.0)
            .execute(&store.pool)
            .await
            .unwrap();
        let policy = store
            .replace_file_protection(&owner, 0, vec![rule()])
            .await
            .unwrap();
        assert!(policy.changed_at.unwrap() > future);
        assert!(
            store.get_turn(&owner, &old.id).await.unwrap().started_at < policy.changed_at.unwrap()
        );
        // Team goal text can be consumed by Alice even when Bob authored it.
        let goal = store
            .create_team_goal(s_code_protocol::CreateTeamGoal {
                scope: scope("bob"),
                title: "Future team goal".into(),
                outcome_definition: "Private goal text".into(),
                target_date: None,
            })
            .await
            .unwrap();
        let goal_future = future + chrono::Duration::days(1);
        sqlx::query("UPDATE team_goals SET created_at=? WHERE id=?")
            .bind(goal_future)
            .bind(&goal.id.0)
            .execute(&store.pool)
            .await
            .unwrap();
        let empty = store
            .replace_file_protection(&owner, 1, vec![])
            .await
            .unwrap();
        assert!(empty.changed_at.unwrap() > goal_future);
        assert!(empty.rules.is_empty());
    }
}
