use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};
use std::{borrow::Cow, path::Path};

async fn verify_privacy_migration(upgrade: bool) {
    // Load the actual directory so this also checks migrations added by other PRs.
    let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let full = Migrator::new(migrations.as_path()).await.unwrap();
    let versions: Vec<_> = full.iter().map(|migration| migration.version).collect();
    assert!(
        versions.windows(2).all(|pair| pair[0] < pair[1]),
        "migration versions must be unique and ordered: {versions:?}"
    );
    assert_eq!(
        full.iter()
            .find(|migration| migration.description == "privacy request indexes")
            .unwrap()
            .version,
        49,
        "version 48 is reserved for the IM channel migration"
    );

    let directory = tempfile::tempdir().unwrap();
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(directory.path().join("migration.sqlite"))
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    if upgrade {
        let previous = Migrator {
            migrations: Cow::Owned(full.iter().filter(|m| m.version < 49).cloned().collect()),
            ..Migrator::DEFAULT
        };
        previous.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO audit_events
             (id, organization_id, team_id, actor_id, event_type, payload_json, created_at, chain_hash)
             VALUES ('existing-event', 'org', 'team', 'actor', 'privacy.request.started',
                     'opaque-existing-payload', '2026-09-20T00:00:00Z', 'existing-hash')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Exercise #81's persisted state when its migration is present as well.
        if has_im_state(&pool).await {
            sqlx::query(
                "INSERT INTO im_channel_state
                 (organization_id, team_id, actor_id, channel, revision, state_json, updated_at) VALUES
                 ('org', 'team', 'actor', 'telegram', 1, '{}', '2026-09-20T00:00:00Z')",
            )
            .execute(&pool)
            .await
            .unwrap();
        }
    }

    full.run(&pool).await.unwrap();
    full.run(&pool).await.unwrap(); // Reapplying the migration set is a no-op.
    let indexes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='index'
         AND name IN ('audit_privacy_starts', 'audit_privacy_outcomes')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(indexes, 2);
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(applied, versions);
    if upgrade {
        let event: (String, String) = sqlx::query_as(
            "SELECT payload_json, chain_hash FROM audit_events WHERE id='existing-event'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            event,
            ("opaque-existing-payload".into(), "existing-hash".into())
        );
        if has_im_state(&pool).await {
            let state: (i64, String) = sqlx::query_as(
                "SELECT revision, state_json FROM im_channel_state WHERE channel='telegram'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(state, (1, "{}".into()));
        }
    }
    pool.close().await;
}

async fn has_im_state(pool: &SqlitePool) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='im_channel_state')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn privacy_migration_fresh_install() {
    verify_privacy_migration(false).await;
}

#[tokio::test]
async fn privacy_migration_upgrade_preserves_existing_data() {
    verify_privacy_migration(true).await;
}
