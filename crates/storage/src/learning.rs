use super::*;
use s_code_protocol::{LearningMode, ProjectLearningSettings, ProjectLesson};

fn canonical_workspace(workspace_uri: &str) -> Result<String, StorageError> {
    let path = url::Url::parse(workspace_uri)
        .ok()
        .and_then(|url| url.to_file_path().ok())
        .or_else(|| {
            Path::new(workspace_uri)
                .is_absolute()
                .then(|| PathBuf::from(workspace_uri))
        })
        .ok_or_else(|| StorageError::InvalidData("invalid learning workspace".into()))?;
    let path = path.canonicalize()?;
    if !path.is_dir() {
        return Err(StorageError::InvalidData(
            "learning workspace is not a directory".into(),
        ));
    }
    url::Url::from_directory_path(path)
        .map(|url| url.to_string())
        .map_err(|_| StorageError::InvalidData("invalid learning workspace".into()))
}

fn project_id(scope: &Scope, workspace_uri: &str) -> Result<String, StorageError> {
    let workspace_uri = canonical_workspace(workspace_uri)?;
    let identity = serde_json::json!([
        scope.organization_id.0,
        scope.team_id.0,
        scope.actor_id.0,
        workspace_uri
    ]);
    Ok(format!(
        "learn_{:x}",
        Sha256::digest(identity.to_string().as_bytes())
    ))
}

fn mode_name(mode: LearningMode) -> &'static str {
    match mode {
        LearningMode::Off => "off",
        LearningMode::Learn => "learn",
        LearningMode::Reuse => "reuse",
    }
}

impl Store {
    pub async fn learning_tool_calls(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Vec<ToolCall>, StorageError> {
        self.get_turn(scope, turn_id).await?;
        let bytes: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(COALESCE(length(arguments_json),0)+COALESCE(length(result_json),0)+COALESCE(length(error),0)),0) FROM (SELECT arguments_json,result_json,error FROM tool_calls WHERE turn_id=? ORDER BY created_at DESC,id DESC LIMIT 96)")
            .bind(&turn_id.0).fetch_one(&self.pool).await?;
        if bytes > 512 * 1024 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query("SELECT * FROM (SELECT * FROM tool_calls WHERE turn_id=? ORDER BY created_at DESC,id DESC LIMIT 96) ORDER BY created_at ASC,id ASC")
            .bind(&turn_id.0).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let call = row_to_tool_call(row, &self.sensitive)?;
                ensure_actor_scope(&call.request.scope, scope)?;
                Ok(call)
            })
            .collect()
    }

    pub async fn learning_user_messages(
        &self,
        scope: &Scope,
        turn_id: &Id,
    ) -> Result<Vec<Message>, StorageError> {
        self.get_turn(scope, turn_id).await?;
        let rows = sqlx::query("SELECT * FROM messages WHERE turn_id=? AND role='user' ORDER BY created_at DESC,id DESC LIMIT 8")
            .bind(&turn_id.0).fetch_all(&self.pool).await?;
        rows.iter()
            .rev()
            .map(|row| row_to_message(row, scope, &self.sensitive))
            .collect()
    }

    pub async fn project_learning_settings(
        &self,
        scope: &Scope,
        workspace_uri: &str,
    ) -> Result<ProjectLearningSettings, StorageError> {
        let row = sqlx::query("SELECT mode,generation FROM learning_projects WHERE id=?")
            .bind(project_id(scope, workspace_uri)?)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Ok(ProjectLearningSettings {
                mode: LearningMode::Off,
                generation: 0,
            }),
            Some(row) => Ok(ProjectLearningSettings {
                mode: match row.try_get::<String, _>("mode")?.as_str() {
                    "learn" => LearningMode::Learn,
                    "reuse" => LearningMode::Reuse,
                    _ => LearningMode::Off,
                },
                generation: row.try_get::<i64, _>("generation")? as u64,
            }),
        }
    }

    pub async fn set_project_learning(
        &self,
        scope: &Scope,
        workspace_uri: &str,
        mode: LearningMode,
    ) -> Result<ProjectLearningSettings, StorageError> {
        sqlx::query("INSERT INTO learning_projects(id,organization_id,team_id,actor_id,mode,generation) VALUES(?,?,?,?,?,1) ON CONFLICT(id) DO UPDATE SET mode=excluded.mode,generation=generation+1")
            .bind(project_id(scope, workspace_uri)?)
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&scope.actor_id.0)
            .bind(mode_name(mode)).execute(&self.pool).await?;
        self.project_learning_settings(scope, workspace_uri).await
    }

    pub async fn list_project_lessons(
        &self,
        scope: &Scope,
        workspace_uri: &str,
    ) -> Result<Vec<ProjectLesson>, StorageError> {
        let rows = sqlx::query("SELECT id,content_json FROM project_lessons WHERE project_id=? AND revoked=0 AND expires_at>? ORDER BY created_at DESC,id LIMIT 64")
            .bind(project_id(scope, workspace_uri)?).bind(Utc::now())
            .fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let id = Id(row.try_get("id")?);
                let content: String = row.try_get("content_json")?;
                let plaintext = self.sensitive.open_text(
                    scope,
                    "project_lessons",
                    &id,
                    "content_json",
                    &content,
                )?;
                serde_json::from_str(&plaintext)
                    .map_err(|e| StorageError::InvalidData(e.to_string()))
            })
            .collect()
    }

    /// Commit only against the generation observed before extraction. A settings
    /// change or removal therefore also revokes any in-flight extraction.
    pub async fn save_project_lessons(
        &self,
        scope: &Scope,
        workspace_uri: &str,
        generation: u64,
        lessons: &[ProjectLesson],
    ) -> Result<usize, StorageError> {
        if lessons.len() > 3 {
            return Err(StorageError::InvalidData(
                "too many lessons in one extraction".into(),
            ));
        }
        for lesson in lessons {
            let source = self.get_turn(scope, &lesson.source_turn_id).await?;
            let session = self.get_session(&lesson.source_session_id).await?;
            ensure_actor_session_scope(&session, scope)?;
            if source.session_id != session.id
                || canonical_workspace(&session.workspace_uri)?
                    != canonical_workspace(workspace_uri)?
            {
                return Err(StorageError::ScopeMismatch);
            }
        }
        let project = project_id(scope, workspace_uri)?;
        let mut transaction = self.pool.begin().await?;
        // Acquire the writer lock before checking the generation; SQLite's
        // deferred read transactions cannot safely upgrade during concurrent writes.
        let eligible = sqlx::query("UPDATE learning_projects SET generation=generation WHERE id=? AND generation=? AND mode='learn'")
            .bind(&project).bind(generation as i64).execute(&mut *transaction).await?.rows_affected();
        if eligible == 0 {
            return Ok(0);
        }
        sqlx::query("UPDATE project_lessons SET revoked=1,content_json=NULL WHERE project_id=? AND expires_at<=?")
            .bind(&project).bind(Utc::now()).execute(&mut *transaction).await?;
        let mut saved = 0;
        for lesson in lessons {
            let content = serde_json::to_string(lesson)
                .map_err(|e| StorageError::InvalidData(e.to_string()))?;
            let content = self.sensitive.seal_text(
                scope,
                "project_lessons",
                &lesson.id,
                "content_json",
                &content,
            )?;
            let key = format!(
                "{:x}",
                Sha256::digest(format!("{}\n{}", lesson.applicability, lesson.guidance).as_bytes())
            );
            saved += sqlx::query("INSERT OR IGNORE INTO project_lessons(id,project_id,content_key,content_json,created_at,expires_at) VALUES(?,?,?,?,?,?)")
                .bind(&lesson.id.0).bind(&project).bind(key).bind(content)
                .bind(lesson.created_at).bind(lesson.expires_at)
                .execute(&mut *transaction).await?.rows_affected() as usize;
        }
        sqlx::query("UPDATE project_lessons SET revoked=1,content_json=NULL WHERE id IN (SELECT id FROM project_lessons WHERE project_id=? AND revoked=0 ORDER BY created_at DESC,id LIMIT -1 OFFSET 64)")
            .bind(&project).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(saved)
    }

    pub async fn revoke_project_lessons(
        &self,
        scope: &Scope,
        workspace_uri: &str,
        lesson_id: Option<&Id>,
    ) -> Result<u64, StorageError> {
        let project = project_id(scope, workspace_uri)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE learning_projects SET generation=generation+1 WHERE id=?")
            .bind(&project)
            .execute(&mut *transaction)
            .await?;
        let removed = sqlx::query("UPDATE project_lessons SET revoked=1,content_json=NULL WHERE project_id=? AND revoked=0 AND (? IS NULL OR id=?)")
            .bind(&project).bind(lesson_id.map(|id| &id.0)).bind(lesson_id.map(|id| &id.0))
            .execute(&mut *transaction).await?.rows_affected();
        transaction.commit().await?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use s_code_protocol::LessonFile;

    fn scope() -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("alice".into()),
            goal_id: None,
            task_id: None,
        }
    }

    async fn fixture() -> (tempfile::TempDir, Store, Scope, String, ProjectLesson) {
        let directory = tempfile::tempdir().unwrap();
        let workspace = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let scope = scope();
        let store = Store::connect_encrypted("sqlite::memory:", "learning-test", &[42_u8; 32])
            .await
            .unwrap();
        let session = store
            .create_session(CreateSession {
                scope: scope.clone(),
                workspace_uri: workspace.clone(),
                title: "Training".into(),
                model: "fixture".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&scope, &session.id).await.unwrap();
        let lesson = ProjectLesson {
            id: Id("lesson-original".into()),
            source_session_id: session.id,
            source_turn_id: turn.id,
            applicability: "queue retries".into(),
            guidance: "Use the shared queue clock for deadlines.".into(),
            evidence_tool_call_ids: vec![Id("read".into()), Id("verify".into())],
            files: vec![LessonFile {
                path: "clock.py".into(),
                sha256: "a".repeat(64),
            }],
            created_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::days(30),
        };
        (directory, store, scope, workspace, lesson)
    }

    #[tokio::test]
    async fn project_learning_is_encrypted_scoped_and_canonical() {
        let (directory, store, owner, workspace, lesson) = fixture().await;
        assert_eq!(
            store
                .project_learning_settings(&owner, &workspace)
                .await
                .unwrap()
                .mode,
            LearningMode::Off
        );
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&lesson)
                )
                .await
                .unwrap(),
            1
        );
        let raw: String = sqlx::query_scalar("SELECT content_json FROM project_lessons")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        assert!(raw.starts_with("enc:v1:"));
        assert!(!raw.contains("shared queue clock"));
        let alias = directory.path().join(".").to_string_lossy().to_string();
        assert_eq!(
            store.list_project_lessons(&owner, &alias).await.unwrap(),
            vec![lesson.clone()]
        );
        for other in [
            Scope {
                actor_id: Id("bob".into()),
                ..owner.clone()
            },
            Scope {
                team_id: Id("other-team".into()),
                ..owner.clone()
            },
            Scope {
                organization_id: Id("other-org".into()),
                ..owner.clone()
            },
        ] {
            assert!(
                store
                    .list_project_lessons(&other, &workspace)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                store
                    .revoke_project_lessons(&other, &workspace, None)
                    .await
                    .unwrap(),
                0
            );
            assert!(
                store
                    .save_project_lessons(
                        &other,
                        &workspace,
                        settings.generation,
                        std::slice::from_ref(&lesson)
                    )
                    .await
                    .is_err()
            );
        }
        let another = tempfile::tempdir().unwrap();
        let other_workspace = another.path().to_string_lossy().to_string();
        assert!(
            store
                .list_project_lessons(&owner, &other_workspace)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .save_project_lessons(&owner, &other_workspace, settings.generation, &[lesson])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn freezing_and_forgetting_revoke_in_flight_work_without_resurrection() {
        let (_directory, store, owner, workspace, lesson) = fixture().await;
        let enabled = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        let frozen = store
            .set_project_learning(&owner, &workspace, LearningMode::Reuse)
            .await
            .unwrap();
        assert!(frozen.generation > enabled.generation);
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    enabled.generation,
                    std::slice::from_ref(&lesson)
                )
                .await
                .unwrap(),
            0
        );
        let enabled = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    enabled.generation,
                    std::slice::from_ref(&lesson)
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .revoke_project_lessons(&owner, &workspace, Some(&lesson.id))
                .await
                .unwrap(),
            1
        );
        let after = store
            .project_learning_settings(&owner, &workspace)
            .await
            .unwrap();
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    enabled.generation,
                    std::slice::from_ref(&lesson)
                )
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, after.generation, &[lesson])
                .await
                .unwrap(),
            0
        );
        let retained: Option<String> =
            sqlx::query_scalar("SELECT content_json FROM project_lessons")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(retained, None);
        assert!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn concurrent_clear_wins_over_earlier_extraction() {
        let (_directory, store, owner, workspace, lesson) = fixture().await;
        let enabled = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        let lessons = [lesson];
        let (saved, cleared) = tokio::join!(
            store.save_project_lessons(&owner, &workspace, enabled.generation, &lessons),
            store.revoke_project_lessons(&owner, &workspace, None)
        );
        saved.unwrap();
        cleared.unwrap();
        assert!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn expired_experience_is_omitted_and_duplicate_content_is_bounded() {
        let (_directory, store, owner, workspace, mut lesson) = fixture().await;
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        lesson.expires_at = Utc::now() - chrono::Duration::days(1);
        store
            .save_project_lessons(
                &owner,
                &workspace,
                settings.generation,
                std::slice::from_ref(&lesson),
            )
            .await
            .unwrap();
        assert!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .is_empty()
        );
        lesson.id = Id("new-lesson".into());
        lesson.expires_at = Utc::now() + chrono::Duration::days(30);
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&lesson)
                )
                .await
                .unwrap(),
            1
        );
        lesson.id = Id("duplicate-lesson".into());
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[lesson])
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
