use super::*;
use s_code_protocol::{
    LearningMode, ProjectLearningOutcome, ProjectLearningSettings, ProjectLesson,
};

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

// Paths are already constrained by daemon evidence checks. This normalizes
// ordering and repeated pairs only, without inventing filesystem alias identity.
fn dependency_set(lesson: &ProjectLesson) -> std::collections::BTreeSet<(&str, &str)> {
    lesson
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.sha256.as_str()))
        .collect()
}

fn validate_source_record(lesson: &ProjectLesson) -> Result<(), StorageError> {
    let Some(observation) = &lesson.source_observation else {
        return Ok(());
    };
    let invalid = || StorageError::InvalidData("invalid source observation".into());
    if lesson.files.len() != 1
        || lesson.files[0].path != observation.path
        || lesson.files[0].sha256 != observation.sha256
        || observation.path.is_empty()
        || observation.path.chars().any(char::is_control)
        || observation
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(&observation.path).is_absolute()
        || observation.sha256.len() != 64
        || !observation
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || observation.start_line == 0
        || observation.end_line < observation.start_line
        || observation.fragments.is_empty()
        || observation.fragments.len() > 2
        || lesson.evidence_tool_call_ids.len() != 2
        || lesson.evidence_tool_call_ids[0] == lesson.evidence_tool_call_ids[1]
        || serde_json::to_vec(lesson).map_err(|_| invalid())?.len() > 3_200
    {
        return Err(invalid());
    }
    let value = serde_json::to_value(lesson).map_err(|_| invalid())?;
    if s_code_audit::redact(value.clone()) != value {
        return Err(invalid());
    }
    let mut previous_end = observation.start_line - 1;
    let mut retained = 0_u64;
    for part in &observation.fragments {
        let lines = part.text.split_inclusive('\n').count() as u64;
        let end = u64::from(part.start_line)
            .checked_add(lines.saturating_sub(1))
            .ok_or_else(invalid)?;
        if lines == 0
            || part.start_line < observation.start_line
            || part.start_line <= previous_end
            || end > u64::from(observation.end_line)
        {
            return Err(invalid());
        }
        previous_end = end as u32;
        retained += lines;
    }
    if !observation.truncated
        && retained != u64::from(observation.end_line - observation.start_line) + 1
    {
        return Err(invalid());
    }
    Ok(())
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
        let project = project_id(scope, workspace_uri)?;
        let row = sqlx::query(
            "SELECT mode,generation,last_outcome_json FROM learning_projects WHERE id=?",
        )
        .bind(&project)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(ProjectLearningSettings {
                mode: LearningMode::Off,
                generation: 0,
                last_outcome: None,
            }),
            Some(row) => Ok(ProjectLearningSettings {
                mode: match row.try_get::<String, _>("mode")?.as_str() {
                    "learn" => LearningMode::Learn,
                    "reuse" => LearningMode::Reuse,
                    _ => LearningMode::Off,
                },
                generation: row.try_get::<i64, _>("generation")? as u64,
                last_outcome: row
                    .try_get::<Option<String>, _>("last_outcome_json")?
                    .map(|content| {
                        let plaintext = self.sensitive.open_text(
                            scope,
                            "learning_projects",
                            &Id(project.clone()),
                            "last_outcome_json",
                            &content,
                        )?;
                        serde_json::from_str(&plaintext)
                            .map_err(|error| StorageError::InvalidData(error.to_string()))
                    })
                    .transpose()?,
            }),
        }
    }

    pub async fn set_project_learning(
        &self,
        scope: &Scope,
        workspace_uri: &str,
        mode: LearningMode,
    ) -> Result<ProjectLearningSettings, StorageError> {
        sqlx::query("INSERT INTO learning_projects(id,organization_id,team_id,actor_id,mode,generation) VALUES(?,?,?,?,?,1) ON CONFLICT(id) DO UPDATE SET mode=excluded.mode,generation=generation+1,last_outcome_json=NULL")
            .bind(project_id(scope, workspace_uri)?)
            .bind(&scope.organization_id.0).bind(&scope.team_id.0).bind(&scope.actor_id.0)
            .bind(mode_name(mode)).execute(&self.pool).await?;
        self.project_learning_settings(scope, workspace_uri).await
    }

    /// Status cannot resurrect work revoked by a mode change or forgetting.
    /// The encrypted payload has no free-form model or project content.
    pub async fn record_project_learning_outcome(
        &self,
        scope: &Scope,
        workspace_uri: &str,
        generation: u64,
        outcome: &ProjectLearningOutcome,
    ) -> Result<bool, StorageError> {
        let source = self.get_turn(scope, &outcome.source_turn_id).await?;
        let session = self.get_session(&source.session_id).await?;
        ensure_actor_session_scope(&session, scope)?;
        if canonical_workspace(&session.workspace_uri)? != canonical_workspace(workspace_uri)? {
            return Err(StorageError::ScopeMismatch);
        }
        let project = project_id(scope, workspace_uri)?;
        let content = serde_json::to_string(outcome)
            .map_err(|error| StorageError::InvalidData(error.to_string()))?;
        let content = self.sensitive.seal_text(
            scope,
            "learning_projects",
            &Id(project.clone()),
            "last_outcome_json",
            &content,
        )?;
        Ok(sqlx::query("UPDATE learning_projects SET last_outcome_json=? WHERE id=? AND generation=? AND mode='learn'")
            .bind(content).bind(project).bind(generation as i64)
            .execute(&self.pool).await?.rows_affected() != 0)
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
            validate_source_record(lesson)?;
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
        let now = Utc::now();
        sqlx::query("UPDATE project_lessons SET revoked=1,content_json=NULL WHERE project_id=? AND expires_at<=?")
            .bind(&project).bind(now).execute(&mut *transaction).await?;
        let mut saved = 0;
        let mut seen_content = HashSet::new();
        for lesson in lessons {
            // An expired proposal or a replayed source/index must never retire
            // a currently usable record, or consume the batch's fresh candidate slot.
            if lesson.expires_at <= now {
                continue;
            }
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_lessons WHERE id=?")
                .bind(&lesson.id.0)
                .fetch_one(&mut *transaction)
                .await?;
            if exists != 0 {
                continue;
            }
            let key = if let Some(observation) = &lesson.source_observation {
                // Cropping a different range of the same version is not new experience.
                // The kind prefix keeps this identity separate from legacy prose keys.
                format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::json!(["source_observation_v1", observation.path])
                            .to_string()
                            .as_bytes()
                    )
                )
            } else {
                format!(
                    "{:x}",
                    Sha256::digest(
                        format!("{}\n{}", lesson.applicability, lesson.guidance).as_bytes()
                    )
                )
            };
            let incumbent = sqlx::query("SELECT id,content_json FROM project_lessons WHERE project_id=? AND content_key=? AND revoked=0")
                .bind(&project).bind(&key).fetch_optional(&mut *transaction).await?;
            let (replacing, unchanged) = if let Some(row) = incumbent {
                let id = Id(row.try_get("id")?);
                let content: String = row.try_get("content_json")?;
                let plaintext = self.sensitive.open_text(
                    scope,
                    "project_lessons",
                    &id,
                    "content_json",
                    &content,
                )?;
                let previous: ProjectLesson = serde_json::from_str(&plaintext)
                    .map_err(|error| StorageError::InvalidData(error.to_string()))?;
                // Only a different verified task can replace an incumbent. Source
                // identity is per file, independent of ranges and descriptive labels.
                if previous.source_turn_id == lesson.source_turn_id {
                    continue;
                }
                let unchanged = match (&previous.source_observation, &lesson.source_observation) {
                    (Some(old), Some(new)) if old.path == new.path => old.sha256 == new.sha256,
                    (None, None)
                        if previous.applicability == lesson.applicability
                            && previous.guidance == lesson.guidance =>
                    {
                        dependency_set(&previous) == dependency_set(lesson)
                    }
                    _ => continue,
                };
                (Some(id), unchanged)
            } else {
                (None, false)
            };
            if !seen_content.insert(key.clone()) || unchanged {
                continue;
            }
            let content = serde_json::to_string(lesson)
                .map_err(|error| StorageError::InvalidData(error.to_string()))?;
            let content = self.sensitive.seal_text(
                scope,
                "project_lessons",
                &lesson.id,
                "content_json",
                &content,
            )?;
            if let Some(id) = replacing {
                sqlx::query("UPDATE project_lessons SET revoked=1,content_json=NULL WHERE id=?")
                    .bind(&id.0)
                    .execute(&mut *transaction)
                    .await?;
            }
            // A failed insertion rolls back retirement and every earlier write
            // in this batch. Never silently ignore an error after retirement.
            saved += sqlx::query("INSERT INTO project_lessons(id,project_id,content_key,content_json,created_at,expires_at) VALUES(?,?,?,?,?,?)")
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
        sqlx::query("UPDATE learning_projects SET generation=generation+1,last_outcome_json=NULL WHERE id=?")
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
            source_observation: None,
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
    async fn existing_learning_settings_upgrade_without_inventing_an_outcome() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let full = sqlx::migrate!("./migrations");
        let previous = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                full.iter()
                    .filter(|migration| migration.version <= 46)
                    .cloned()
                    .collect(),
            ),
            ..sqlx::migrate::Migrator::DEFAULT
        };
        previous.run(&pool).await.unwrap();
        sqlx::query("INSERT INTO learning_projects(id,organization_id,team_id,actor_id,mode,generation) VALUES('project','org','team','actor','learn',7)").execute(&pool).await.unwrap();
        full.run(&pool).await.unwrap();
        let row = sqlx::query(
            "SELECT mode,generation,last_outcome_json FROM learning_projects WHERE id='project'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("mode"), "learn");
        assert_eq!(row.get::<i64, _>("generation"), 7);
        assert!(row.get::<Option<String>, _>("last_outcome_json").is_none());
    }

    #[tokio::test]
    async fn learning_outcome_is_encrypted_scoped_and_revoked_with_inflight_work() {
        use s_code_protocol::{ProjectLearningReason, ProjectLearningStatus};
        for change in ["clear", "remove", "off", "reuse"] {
            let (directory, store, owner, workspace, lesson) = fixture().await;
            let enabled = store
                .set_project_learning(&owner, &workspace, LearningMode::Learn)
                .await
                .unwrap();
            let outcome = ProjectLearningOutcome {
                status: ProjectLearningStatus::Skipped,
                reason: ProjectLearningReason::ChangesAfterVerification,
                saved_count: 0,
                recorded_at: Utc::now(),
                source_turn_id: lesson.source_turn_id.clone(),
            };
            assert!(
                store
                    .record_project_learning_outcome(
                        &owner,
                        &workspace,
                        enabled.generation,
                        &outcome
                    )
                    .await
                    .unwrap()
            );
            let alias = directory.path().join(".").to_string_lossy().to_string();
            assert_eq!(
                store
                    .project_learning_settings(&owner, &alias)
                    .await
                    .unwrap()
                    .last_outcome,
                Some(outcome.clone())
            );
            let encrypted: String =
                sqlx::query_scalar("SELECT last_outcome_json FROM learning_projects WHERE id=?")
                    .bind(project_id(&owner, &workspace).unwrap())
                    .fetch_one(&store.pool)
                    .await
                    .unwrap();
            assert!(encrypted.starts_with("enc:v1:"));
            assert!(!encrypted.contains("changes_after_verification"));
            let other_actor = Scope {
                actor_id: Id("bob".into()),
                ..owner.clone()
            };
            assert!(
                store
                    .project_learning_settings(&other_actor, &workspace)
                    .await
                    .unwrap()
                    .last_outcome
                    .is_none()
            );
            assert!(
                store
                    .record_project_learning_outcome(
                        &other_actor,
                        &workspace,
                        enabled.generation,
                        &outcome
                    )
                    .await
                    .is_err()
            );
            let other_project = tempfile::tempdir().unwrap();
            assert!(
                store
                    .record_project_learning_outcome(
                        &owner,
                        &other_project.path().to_string_lossy(),
                        enabled.generation,
                        &outcome
                    )
                    .await
                    .is_err()
            );
            match change {
                "clear" => {
                    store
                        .revoke_project_lessons(&owner, &workspace, None)
                        .await
                        .unwrap();
                }
                "remove" => {
                    store
                        .revoke_project_lessons(&owner, &workspace, Some(&lesson.id))
                        .await
                        .unwrap();
                }
                "off" => {
                    store
                        .set_project_learning(&owner, &workspace, LearningMode::Off)
                        .await
                        .unwrap();
                }
                _ => {
                    store
                        .set_project_learning(&owner, &workspace, LearningMode::Reuse)
                        .await
                        .unwrap();
                }
            }
            assert!(
                store
                    .project_learning_settings(&owner, &workspace)
                    .await
                    .unwrap()
                    .last_outcome
                    .is_none()
            );
            assert!(
                !store
                    .record_project_learning_outcome(
                        &owner,
                        &workspace,
                        enabled.generation,
                        &outcome
                    )
                    .await
                    .unwrap()
            );
            let fresh = store
                .set_project_learning(&owner, &workspace, LearningMode::Learn)
                .await
                .unwrap();
            assert!(
                !store
                    .record_project_learning_outcome(
                        &owner,
                        &workspace,
                        enabled.generation,
                        &outcome
                    )
                    .await
                    .unwrap()
            );
            assert!(
                store
                    .record_project_learning_outcome(&owner, &workspace, fresh.generation, &outcome)
                    .await
                    .unwrap()
            );
            assert_eq!(
                store
                    .project_learning_settings(&owner, &workspace)
                    .await
                    .unwrap()
                    .last_outcome,
                Some(outcome)
            );
        }
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

    // Storage accepts already validated evidence; each refresh fixture has a
    // genuinely separate source turn rather than merely a different lesson id.
    async fn fresh_source_lesson(
        store: &Store,
        owner: &Scope,
        base: &ProjectLesson,
        id: &str,
        hash: &str,
    ) -> ProjectLesson {
        let turn = store
            .create_turn(owner, &base.source_session_id)
            .await
            .unwrap();
        store
            .update_turn(owner, &turn.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();
        let mut lesson = base.clone();
        lesson.id = Id(id.into());
        lesson.source_turn_id = turn.id;
        lesson.files[0].sha256 = hash.repeat(64);
        lesson.created_at = Utc::now();
        lesson.expires_at = lesson.created_at + chrono::Duration::days(30);
        lesson
    }

    async fn stored_lesson_payload(store: &Store, id: &Id) -> (i64, Option<String>) {
        let row = sqlx::query("SELECT revoked,content_json FROM project_lessons WHERE id=?")
            .bind(&id.0)
            .fetch_one(&store.pool)
            .await
            .unwrap();
        (row.get("revoked"), row.get("content_json"))
    }

    #[tokio::test]
    async fn new_source_refreshes_dependency_evidence_without_resurrecting_old_payload() {
        let (_directory, store, owner, workspace, old) = fixture().await;
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        store
            .update_turn(
                &owner,
                &old.source_turn_id,
                TurnStatus::Completed,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&old)
                )
                .await
                .unwrap(),
            1
        );
        let old_ciphertext = stored_lesson_payload(&store, &old.id).await.1.unwrap();
        let fresh = fresh_source_lesson(&store, &owner, &old, "fresh-version", "b").await;
        assert_ne!(old.source_turn_id, fresh.source_turn_id);
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&fresh)
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(stored_lesson_payload(&store, &old.id).await, (1, None));
        let (revoked, content) = stored_lesson_payload(&store, &fresh.id).await;
        let content = content.unwrap();
        assert_eq!(revoked, 0);
        assert!(content.starts_with("enc:v1:"));
        assert!(!content.contains(&fresh.guidance));
        assert_ne!(content, old_ciphertext);
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![fresh]
        );
    }

    #[tokio::test]
    async fn identical_or_reordered_dependency_sets_remain_duplicates() {
        let (_directory, store, owner, workspace, mut old) = fixture().await;
        old.files.push(LessonFile {
            path: "queue.py".into(),
            sha256: "c".repeat(64),
        });
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        store
            .save_project_lessons(
                &owner,
                &workspace,
                settings.generation,
                std::slice::from_ref(&old),
            )
            .await
            .unwrap();
        let original = stored_lesson_payload(&store, &old.id).await;
        for variant in ["same", "reordered", "repeated"] {
            let mut fresh = fresh_source_lesson(&store, &owner, &old, variant, "a").await;
            if variant != "same" {
                fresh.files.reverse();
            }
            if variant == "repeated" {
                fresh.files.push(fresh.files[0].clone());
            }
            assert_eq!(
                store
                    .save_project_lessons(&owner, &workspace, settings.generation, &[fresh])
                    .await
                    .unwrap(),
                0,
                "{variant}"
            );
            assert_eq!(stored_lesson_payload(&store, &old.id).await, original);
        }
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![old]
        );
    }

    #[tokio::test]
    async fn stale_generation_old_ids_same_source_and_expired_candidates_cannot_retire_current_evidence()
     {
        let (_directory, store, owner, workspace, old) = fixture().await;
        let before = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        store
            .save_project_lessons(
                &owner,
                &workspace,
                before.generation,
                std::slice::from_ref(&old),
            )
            .await
            .unwrap();
        let fresh = fresh_source_lesson(&store, &owner, &old, "current-version", "b").await;
        store
            .set_project_learning(&owner, &workspace, LearningMode::Reuse)
            .await
            .unwrap();
        let after = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    before.generation,
                    std::slice::from_ref(&fresh)
                )
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![old.clone()]
        );
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    after.generation,
                    std::slice::from_ref(&fresh)
                )
                .await
                .unwrap(),
            1
        );
        let original = stored_lesson_payload(&store, &fresh.id).await;
        for variant in ["old_id", "active_id", "same_source", "expired"] {
            let mut replay = fresh_source_lesson(&store, &owner, &fresh, variant, "c").await;
            match variant {
                "old_id" => replay.id = old.id.clone(),
                "active_id" => replay.id = fresh.id.clone(),
                "same_source" => replay.source_turn_id = fresh.source_turn_id.clone(),
                "expired" => replay.expires_at = Utc::now() - chrono::Duration::seconds(1),
                _ => unreachable!(),
            }
            assert_eq!(
                store
                    .save_project_lessons(&owner, &workspace, after.generation, &[replay])
                    .await
                    .unwrap(),
                0,
                "{variant}"
            );
            assert_eq!(stored_lesson_payload(&store, &fresh.id).await, original);
            assert_eq!(stored_lesson_payload(&store, &old.id).await, (1, None));
        }
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![fresh]
        );
    }

    #[tokio::test]
    async fn one_batch_keeps_first_fresh_candidate_without_counting_replacements_twice() {
        for invalid_first in ["replay", "expired", "same_source"] {
            let (_directory, store, owner, workspace, old) = fixture().await;
            let settings = store
                .set_project_learning(&owner, &workspace, LearningMode::Learn)
                .await
                .unwrap();
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&old),
                )
                .await
                .unwrap();
            let mut ignored = fresh_source_lesson(&store, &owner, &old, "ignored", "d").await;
            match invalid_first {
                "replay" => ignored.id = old.id.clone(),
                "expired" => ignored.expires_at = Utc::now() - chrono::Duration::seconds(1),
                _ => ignored.source_turn_id = old.source_turn_id.clone(),
            }
            let first = fresh_source_lesson(&store, &owner, &old, "first-fresh", "b").await;
            let second = fresh_source_lesson(&store, &owner, &old, "later-fresh", "c").await;
            assert_eq!(
                store
                    .save_project_lessons(
                        &owner,
                        &workspace,
                        settings.generation,
                        &[ignored, first.clone(), second]
                    )
                    .await
                    .unwrap(),
                1,
                "{invalid_first}"
            );
            assert_eq!(
                store
                    .list_project_lessons(&owner, &workspace)
                    .await
                    .unwrap(),
                vec![first]
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_lessons")
                    .fetch_one(&store.pool)
                    .await
                    .unwrap(),
                2
            );
        }
    }

    #[tokio::test]
    async fn legacy_key_field_boundary_collision_cannot_replace_different_text() {
        let (_directory, store, owner, workspace, mut old) = fixture().await;
        old.applicability = "queue\nclock".into();
        old.guidance = "deadline convention".into();
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        store
            .save_project_lessons(
                &owner,
                &workspace,
                settings.generation,
                std::slice::from_ref(&old),
            )
            .await
            .unwrap();
        let original = stored_lesson_payload(&store, &old.id).await;
        let mut different =
            fresh_source_lesson(&store, &owner, &old, "field-boundary-collision", "b").await;
        different.applicability = "queue".into();
        different.guidance = "clock\ndeadline convention".into();
        assert_eq!(
            format!("{}\n{}", old.applicability, old.guidance),
            format!("{}\n{}", different.applicability, different.guidance)
        );
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[different])
                .await
                .unwrap(),
            0
        );
        assert_eq!(stored_lesson_payload(&store, &old.id).await, original);
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![old]
        );
    }

    #[tokio::test]
    async fn failed_refresh_insertion_rolls_back_every_retirement_and_prior_batch_insert() {
        let (_directory, store, owner, workspace, old_a) = fixture().await;
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        let mut old_b = fresh_source_lesson(&store, &owner, &old_a, "old-b", "a").await;
        old_b.guidance = "Validate queue inputs before opening a transaction.".into();
        store
            .save_project_lessons(
                &owner,
                &workspace,
                settings.generation,
                &[old_a.clone(), old_b.clone()],
            )
            .await
            .unwrap();
        let before_a = stored_lesson_payload(&store, &old_a.id).await;
        let before_b = stored_lesson_payload(&store, &old_b.id).await;
        let new_a = fresh_source_lesson(&store, &owner, &old_a, "new-a", "b").await;
        let new_b = fresh_source_lesson(&store, &owner, &old_b, "reject-new-b", "b").await;
        sqlx::query("CREATE TRIGGER reject_refresh_fixture BEFORE INSERT ON project_lessons WHEN NEW.id='reject-new-b' BEGIN SELECT RAISE(ABORT,'fixture insertion failure'); END").execute(&store.pool).await.unwrap();
        assert!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[new_a, new_b])
                .await
                .is_err()
        );
        assert_eq!(stored_lesson_payload(&store, &old_a.id).await, before_a);
        assert_eq!(stored_lesson_payload(&store, &old_b.id).await, before_b);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_lessons")
                .fetch_one(&store.pool)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn expired_experience_is_omitted_and_duplicate_content_is_bounded() {
        let (_directory, store, owner, workspace, mut lesson) = fixture().await;
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
        // Age an actually persisted incumbent, including its encrypted payload,
        // rather than passing an already-expired proposal that should be skipped.
        lesson.created_at = Utc::now() - chrono::Duration::days(31);
        lesson.expires_at = Utc::now() - chrono::Duration::days(1);
        let content = store
            .sensitive
            .seal_text(
                &owner,
                "project_lessons",
                &lesson.id,
                "content_json",
                &serde_json::to_string(&lesson).unwrap(),
            )
            .unwrap();
        sqlx::query(
            "UPDATE project_lessons SET content_json=?,created_at=?,expires_at=? WHERE id=?",
        )
        .bind(content)
        .bind(lesson.created_at)
        .bind(lesson.expires_at)
        .bind(&lesson.id.0)
        .execute(&store.pool)
        .await
        .unwrap();
        assert_eq!(stored_lesson_payload(&store, &lesson.id).await.0, 0);
        assert!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .is_empty()
        );
        let fresh = fresh_source_lesson(&store, &owner, &lesson, "new-lesson", "a").await;
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&fresh)
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(stored_lesson_payload(&store, &lesson.id).await, (1, None));
        let duplicate = fresh_source_lesson(&store, &owner, &fresh, "duplicate-lesson", "a").await;
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[duplicate])
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap(),
            vec![fresh]
        );
    }

    fn typed_observation(mut lesson: ProjectLesson, start: u32, text: &str) -> ProjectLesson {
        lesson.source_observation = Some(s_code_protocol::ProjectSourceObservation {
            path: lesson.files[0].path.clone(),
            sha256: lesson.files[0].sha256.clone(),
            start_line: start,
            end_line: start + text.split_inclusive('\n').count() as u32 - 1,
            fragments: vec![s_code_protocol::SourceFragment {
                start_line: start,
                text: text.into(),
            }],
            truncated: false,
        });
        lesson
    }

    #[tokio::test]
    async fn source_identity_uses_path_version_not_crop_or_generated_guidance() {
        let (_directory, store, owner, workspace, base) = fixture().await;
        store
            .update_turn(
                &owner,
                &base.source_turn_id,
                TurnStatus::Completed,
                None,
                None,
            )
            .await
            .unwrap();
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        let source = typed_observation(base.clone(), 1, "def queue_deadline():\n    return 1\n");
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&source)
                )
                .await
                .unwrap(),
            1
        );
        let payload = stored_lesson_payload(&store, &source.id).await.1.unwrap();
        assert!(!payload.contains("queue_deadline"));
        let mut new_crop = typed_observation(
            fresh_source_lesson(&store, &owner, &source, "different-crop", "a").await,
            22,
            "other source range\n",
        );
        new_crop.guidance = "Different compatibility display text".into();
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[new_crop])
                .await
                .unwrap(),
            0
        );
        let mut same_turn = source.clone();
        same_turn.id = Id("same-turn-new-id".into());
        same_turn.files[0].sha256 = "b".repeat(64);
        same_turn.source_observation.as_mut().unwrap().sha256 = "b".repeat(64);
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[same_turn])
                .await
                .unwrap(),
            0
        );
        let next = typed_observation(
            fresh_source_lesson(&store, &owner, &source, "new-verified-version", "b").await,
            1,
            "def queue_deadline():\n    return 2\n",
        );
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    std::slice::from_ref(&next)
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(stored_lesson_payload(&store, &source.id).await, (1, None));
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[source])
                .await
                .unwrap(),
            0
        );
        let active = store
            .list_project_lessons(&owner, &workspace)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, next.id);
        // The legacy key is separate; switching representation does not erase history.
        let mut legacy = base;
        legacy.id = Id("old-distilled-view-only".into());
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[legacy])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn source_batches_do_not_inflate_counts_or_resurrect_after_clear() {
        let (_directory, store, owner, workspace, base) = fixture().await;
        store
            .update_turn(
                &owner,
                &base.source_turn_id,
                TurnStatus::Completed,
                None,
                None,
            )
            .await
            .unwrap();
        let settings = store
            .set_project_learning(&owner, &workspace, LearningMode::Learn)
            .await
            .unwrap();
        let source = typed_observation(base, 1, "first source\n");
        let second = typed_observation(
            fresh_source_lesson(&store, &owner, &source, "second-version", "b").await,
            2,
            "second source\n",
        );
        assert_eq!(
            store
                .save_project_lessons(
                    &owner,
                    &workspace,
                    settings.generation,
                    &[source.clone(), second.clone()]
                )
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()[0]
                .id,
            source.id
        );
        store
            .revoke_project_lessons(&owner, &workspace, None)
            .await
            .unwrap();
        assert_eq!(stored_lesson_payload(&store, &source.id).await, (1, None));
        assert_eq!(
            store
                .save_project_lessons(&owner, &workspace, settings.generation, &[second])
                .await
                .unwrap(),
            0
        );
        assert!(
            store
                .list_project_lessons(&owner, &workspace)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn malformed_source_structure_or_sensitive_metadata_cannot_partially_save() {
        for case in [
            "path", "hash", "line", "overlap", "size", "secret", "control",
        ] {
            let (_directory, store, owner, workspace, base) = fixture().await;
            store
                .update_turn(
                    &owner,
                    &base.source_turn_id,
                    TurnStatus::Completed,
                    None,
                    None,
                )
                .await
                .unwrap();
            let settings = store
                .set_project_learning(&owner, &workspace, LearningMode::Learn)
                .await
                .unwrap();
            let good = typed_observation(base, 1, "valid source\n");
            let mut bad = good.clone();
            bad.id = Id("malformed-source".into());
            let observation = bad.source_observation.as_mut().unwrap();
            match case {
                "path" => observation.path = "../outside".into(),
                "hash" => observation.sha256 = "b".repeat(64),
                "line" => observation.fragments[0].start_line = 2,
                "overlap" => observation.fragments.push(observation.fragments[0].clone()),
                "size" => observation.fragments[0].text = "x".repeat(3_201),
                "secret" => bad.guidance = ["ghp_", &"x".repeat(36)].concat(),
                "control" => {
                    observation.path = "clock\nfile.py".into();
                    bad.files[0].path = observation.path.clone();
                }
                _ => unreachable!(),
            }
            assert!(
                store
                    .save_project_lessons(&owner, &workspace, settings.generation, &[good, bad])
                    .await
                    .is_err(),
                "{case}"
            );
            assert!(
                store
                    .list_project_lessons(&owner, &workspace)
                    .await
                    .unwrap()
                    .is_empty(),
                "{case}"
            );
        }
    }
}
