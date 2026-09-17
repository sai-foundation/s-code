//! Durable, account-scoped channel state. Sensitive payloads use the store's
//! encryption policy, and revisions prevent concurrent writers losing updates.

use super::{Id, Scope, StorageError, Store};
use chrono::Utc;
use serde_json::Value;
use sqlx::Row;

const MAX_STATE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ImStateRecord {
    pub revision: i64,
    pub value: Value,
}

fn validate_channel(channel: &str) -> Result<(), StorageError> {
    if channel.is_empty()
        || channel.len() > 64
        || !channel.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
    {
        return Err(StorageError::InvalidData("invalid IM channel".into()));
    }
    Ok(())
}

// The codec already binds organization and team. Including a byte-length-prefixed
// actor here also authenticates actor ownership without ambiguous delimiters.
// Keep the matching legacy preflight SQL projection in lib.rs in sync.
fn record_id(scope: &Scope, channel: &str) -> Id {
    Id(format!(
        "{}:{}:{channel}",
        scope.actor_id.0.len(),
        scope.actor_id.0
    ))
}

impl Store {
    pub async fn get_im_state(
        &self,
        scope: &Scope,
        channel: &str,
    ) -> Result<Option<ImStateRecord>, StorageError> {
        validate_channel(channel)?;
        let row = sqlx::query(
            "SELECT revision,state_json FROM im_channel_state \
             WHERE organization_id=? AND team_id=? AND actor_id=? AND channel=?",
        )
        .bind(&scope.organization_id.0)
        .bind(&scope.team_id.0)
        .bind(&scope.actor_id.0)
        .bind(channel)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let plaintext = self.sensitive.open_text(
            scope,
            "im_channel_state",
            &record_id(scope, channel),
            "state_json",
            &row.try_get::<String, _>("state_json")?,
        )?;
        if plaintext.len() > MAX_STATE_BYTES {
            return Err(StorageError::InvalidData(
                "IM state exceeds size limit".into(),
            ));
        }
        let value = serde_json::from_str(&plaintext)
            .map_err(|_| StorageError::InvalidData("invalid IM state JSON".into()))?;
        Ok(Some(ImStateRecord {
            revision: row.try_get("revision")?,
            value,
        }))
    }

    /// `None` creates an absent channel; `Some(revision)` replaces exactly that
    /// revision. A stale revision returns `InvalidState` and leaves data intact.
    pub async fn put_im_state(
        &self,
        scope: &Scope,
        channel: &str,
        expected_revision: Option<i64>,
        value: &Value,
    ) -> Result<ImStateRecord, StorageError> {
        validate_channel(channel)?;
        let revision = match expected_revision {
            None => 1,
            Some(previous) if previous > 0 && previous < i64::MAX => previous + 1,
            Some(_) => {
                return Err(StorageError::InvalidState(
                    "invalid IM state revision".into(),
                ));
            }
        };
        let plaintext = serde_json::to_string(value)
            .map_err(|_| StorageError::InvalidData("invalid IM state JSON".into()))?;
        if plaintext.len() > MAX_STATE_BYTES {
            return Err(StorageError::InvalidData(
                "IM state exceeds size limit".into(),
            ));
        }
        let encrypted = self.sensitive.seal_text(
            scope,
            "im_channel_state",
            &record_id(scope, channel),
            "state_json",
            &plaintext,
        )?;
        let changed = if let Some(previous) = expected_revision {
            sqlx::query(
                "UPDATE im_channel_state SET revision=?,state_json=?,updated_at=? \
                 WHERE organization_id=? AND team_id=? AND actor_id=? AND channel=? AND revision=?",
            )
            .bind(revision)
            .bind(&encrypted)
            .bind(Utc::now().to_rfc3339())
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&scope.actor_id.0)
            .bind(channel)
            .bind(previous)
            .execute(&self.pool)
            .await?
            .rows_affected()
        } else {
            sqlx::query(
                "INSERT INTO im_channel_state \
                 (organization_id,team_id,actor_id,channel,revision,state_json,updated_at) \
                 VALUES(?,?,?,?,1,?,?) ON CONFLICT(organization_id,team_id,actor_id,channel) DO NOTHING",
            )
            .bind(&scope.organization_id.0)
            .bind(&scope.team_id.0)
            .bind(&scope.actor_id.0)
            .bind(channel)
            .bind(&encrypted)
            .bind(Utc::now().to_rfc3339())
            .execute(&self.pool)
            .await?
            .rows_affected()
        };
        if changed != 1 {
            return Err(StorageError::InvalidState(
                "IM state revision conflict".into(),
            ));
        }
        Ok(ImStateRecord {
            revision,
            value: value.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(
            dir.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        dir
    }

    fn scope(actor: &str) -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id(actor.into()),
            goal_id: None,
            task_id: None,
        }
    }

    #[tokio::test]
    async fn im_encrypted_persistence_and_account_isolation() {
        let dir = private_tempdir();
        let path = dir.path().join("state.sqlite");
        let url = format!("sqlite://{}", path.display());
        let store = Store::connect_encrypted(&url, "test", &[42; 32])
            .await
            .unwrap();
        let value = json!({"bot_token":"sensitive-telegram-token-123456", "offset":31});
        let created = store
            .put_im_state(&scope("alice"), "telegram", None, &value)
            .await
            .unwrap();
        assert_eq!(created.revision, 1);
        assert_eq!(
            store.get_im_state(&scope("bob"), "telegram").await.unwrap(),
            None
        );
        let mut other_team = scope("alice");
        other_team.team_id = Id("other-team".into());
        assert_eq!(
            store.get_im_state(&other_team, "telegram").await.unwrap(),
            None
        );
        let mut other_org = scope("alice");
        other_org.organization_id = Id("other-org".into());
        assert_eq!(
            store.get_im_state(&other_org, "telegram").await.unwrap(),
            None
        );
        assert_eq!(
            store.get_im_state(&scope("alice"), "slack").await.unwrap(),
            None
        );
        let stored: String = sqlx::query_scalar("SELECT state_json FROM im_channel_state")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(stored.starts_with("enc:v1:test:"));
        assert!(!stored.contains("sensitive-telegram-token"));
        store.pool.close().await;
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes
                .windows(b"sensitive-telegram-token".len())
                .any(|window| window == b"sensitive-telegram-token")
        );
        let reopened = Store::connect_encrypted(&url, "test", &[42; 32])
            .await
            .unwrap();
        assert_eq!(
            reopened
                .get_im_state(&scope("alice"), "telegram")
                .await
                .unwrap(),
            Some(created)
        );
        reopened.pool.close().await;
    }

    #[tokio::test]
    async fn im_ciphertext_cannot_be_reassigned_to_another_actor_or_channel() {
        let store = Store::connect_encrypted("sqlite::memory:", "test", &[42; 32])
            .await
            .unwrap();
        store
            .put_im_state(
                &scope("alice"),
                "telegram",
                None,
                &json!({"secret":"value"}),
            )
            .await
            .unwrap();
        sqlx::query("UPDATE im_channel_state SET actor_id='bob'")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(matches!(
            store.get_im_state(&scope("bob"), "telegram").await,
            Err(StorageError::Encryption(_))
        ));
        sqlx::query("UPDATE im_channel_state SET actor_id='alice',channel='slack'")
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(matches!(
            store.get_im_state(&scope("alice"), "slack").await,
            Err(StorageError::Encryption(_))
        ));
    }

    #[tokio::test]
    async fn im_cas_rejects_stale_writes_and_has_one_concurrent_winner() {
        let store = Store::connect_encrypted("sqlite::memory:", "test", &[42; 32])
            .await
            .unwrap();
        let account = scope("alice");
        let value = json!({"offset":1});
        assert!(matches!(
            store
                .put_im_state(&account, "telegram", Some(1), &value)
                .await,
            Err(StorageError::InvalidState(_))
        ));
        store
            .put_im_state(&account, "telegram", None, &value)
            .await
            .unwrap();
        assert!(matches!(
            store.put_im_state(&account, "telegram", None, &value).await,
            Err(StorageError::InvalidState(_))
        ));
        let next_a = json!({"offset":2});
        let next_b = json!({"offset":3});
        let (a, b) = tokio::join!(
            store.put_im_state(&account, "telegram", Some(1), &next_a),
            store.put_im_state(&account, "telegram", Some(1), &next_b),
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let winner = a.or(b).unwrap();
        assert_eq!(winner.revision, 2);
        assert!(matches!(
            store
                .put_im_state(&account, "telegram", Some(1), &value)
                .await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(
            store.get_im_state(&account, "telegram").await.unwrap(),
            Some(winner)
        );
    }

    #[tokio::test]
    async fn im_rejects_unbounded_inputs_and_invalid_revisions() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let account = scope("alice");
        for channel in ["", "Telegram", "foo/bar", "foo bar", &"a".repeat(65)] {
            assert!(matches!(
                store.get_im_state(&account, channel).await,
                Err(StorageError::InvalidData(_))
            ));
            assert!(matches!(
                store
                    .put_im_state(&account, channel, None, &json!({}))
                    .await,
                Err(StorageError::InvalidData(_))
            ));
        }
        for revision in [-1, 0, i64::MAX] {
            assert!(matches!(
                store
                    .put_im_state(&account, "telegram", Some(revision), &json!({}))
                    .await,
                Err(StorageError::InvalidState(_))
            ));
        }
        assert!(matches!(
            store
                .put_im_state(
                    &account,
                    "telegram",
                    None,
                    &json!("a".repeat(MAX_STATE_BYTES))
                )
                .await,
            Err(StorageError::InvalidData(_))
        ));
        assert!(
            store
                .get_im_state(&account, "telegram")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn im_migration_upgrades_existing_database() {
        let dir = private_tempdir();
        let path = dir.path().join("legacy.sqlite");
        let url = format!("sqlite://{}", path.display());
        let options = url
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let migrations = sqlx::migrate!("./migrations");
        let legacy = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                migrations
                    .iter()
                    .filter(|migration| migration.version < 47)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        legacy.run(&pool).await.unwrap();
        pool.close().await;
        let store = Store::connect_encrypted(&url, "test", &[42; 32])
            .await
            .unwrap();
        store
            .put_im_state(&scope("alice"), "telegram", None, &json!({"paired":true}))
            .await
            .unwrap();
        store.verify_integrity().await.unwrap();
        store.pool.close().await;
    }

    #[tokio::test]
    async fn im_unmarked_preflight_authenticates_actor_and_rejects_plaintext() {
        let dir = private_tempdir();
        let url = format!("sqlite://{}", dir.path().join("preflight.sqlite").display());
        let store = Store::connect_encrypted(&url, "test", &[42; 32])
            .await
            .unwrap();
        // Unicode actor exercises SQLite byte length matching the Rust record ID.
        let account = scope("用户:alice");
        store
            .put_im_state(&account, "telegram", None, &json!({"paired":true}))
            .await
            .unwrap();
        sqlx::query("DELETE FROM storage_encryption_metadata")
            .execute(&store.pool)
            .await
            .unwrap();
        store.pool.close().await;
        let store = Store::connect_encrypted(&url, "test", &[42; 32])
            .await
            .unwrap();
        assert!(
            store
                .get_im_state(&account, "telegram")
                .await
                .unwrap()
                .is_some()
        );
        sqlx::query("DELETE FROM storage_encryption_metadata")
            .execute(&store.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE im_channel_state SET state_json='{}'")
            .execute(&store.pool)
            .await
            .unwrap();
        store.pool.close().await;
        assert!(matches!(
            Store::connect_encrypted(&url, "test", &[42; 32]).await,
            Err(StorageError::Encryption(_))
        ));
    }
}
