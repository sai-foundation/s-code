use super::{
    Id, Scope, StorageError, Store, Turn, TurnStatus, ensure_actor_scope, is_terminal, row_to_turn,
};
use chrono::Utc;

impl Store {
    /// Stop a paused turn whose live cancellation token has already closed.
    /// Running turns must be cancelled through their live execution owner.
    pub async fn cancel_paused_turn(&self, scope: &Scope, id: &Id) -> Result<Turn, StorageError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row = sqlx::query("SELECT * FROM turns WHERE id=?")
            .bind(&id.0)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or(StorageError::NotFound)?;
        let mut turn = row_to_turn(&row, &self.sensitive)?;
        ensure_actor_scope(&turn.scope, scope)?;
        if is_terminal(&turn.status) {
            transaction.commit().await?;
            return Ok(turn);
        }
        if !matches!(
            turn.status,
            TurnStatus::AwaitingApproval | TurnStatus::AwaitingInput
        ) {
            return Err(StorageError::InvalidState(
                "turn is no longer paused".into(),
            ));
        }
        // Approval resolution claims the operation by changing pending to
        // approved before executing it. Serialize this check with that claim:
        // either cancellation expires the pending approval first, or the
        // already claimed operation must be stopped by its execution owner.
        let executing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_calls c WHERE c.turn_id=? AND \
             (c.status='running' OR (c.status IN ('proposed','awaiting_approval') AND \
             EXISTS (SELECT 1 FROM approvals a WHERE a.tool_call_id=c.id AND a.status='approved')))",
        )
        .bind(&id.0)
        .fetch_one(&mut *transaction)
        .await?;
        if executing != 0 {
            return Err(StorageError::InvalidState(
                "approved operation already executing".into(),
            ));
        }
        let now = Utc::now();
        let changed = sqlx::query(
            "UPDATE turns SET status='cancelled',checkpoint_json=NULL,error_code=NULL,updated_at=?,completed_at=? \
             WHERE id=? AND status IN ('awaiting_approval','awaiting_input')",
        ).bind(now).bind(now).bind(&id.0).execute(&mut *transaction).await?;
        if changed.rows_affected() != 1 {
            return Err(StorageError::InvalidState(
                "turn is no longer paused".into(),
            ));
        }
        sqlx::query(
            "UPDATE approvals SET status='expired',decided_at=? \
             WHERE status='pending' AND tool_call_id IN (SELECT id FROM tool_calls WHERE turn_id=?)",
        ).bind(now).bind(&id.0).execute(&mut *transaction).await?;
        sqlx::query(
            "UPDATE tool_calls SET status='cancelled',updated_at=? \
             WHERE turn_id=? AND status IN ('proposed','awaiting_approval','running')",
        )
        .bind(now)
        .bind(&id.0)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE question_requests SET status='cancelled',revision=revision+1 WHERE turn_id=? AND status='pending'")
            .bind(&id.0).execute(&mut *transaction).await?;
        sqlx::query(
            "UPDATE turn_inputs SET status='cancelled',updated_at=?,cancelled_at=?,revision=revision+1 \
             WHERE target_turn_id=? AND status IN ('pending','processing')",
        ).bind(now).bind(now).bind(&id.0).execute(&mut *transaction).await?;
        transaction.commit().await?;
        turn.status = TurnStatus::Cancelled;
        turn.checkpoint = None;
        turn.error_code = None;
        turn.updated_at = now;
        turn.completed_at = Some(now);
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolPolicyMetadata;
    use s_code_protocol::{
        ApprovalScope, ApprovalStatus, CreateSession, CreateTurnInput, PolicyDecision,
        PolicyResult, QuestionRequest, QuestionStatus, ToolCallStatus, ToolRequest, TurnInputMode,
        TurnInputStatus,
    };
    use serde_json::json;

    fn scope() -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("alice".into()),
            goal_id: None,
            task_id: None,
        }
    }

    async fn paused(store: &Store, status: TurnStatus) -> Turn {
        let session = store
            .create_session(CreateSession {
                scope: scope(),
                mode: s_code_protocol::SessionMode::Work,
                workspace_uri: "file:///repo".into(),
                title: "Test".into(),
                model: "mock".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&scope(), &session.id).await.unwrap();
        store
            .update_turn(
                &scope(),
                &turn.id,
                status,
                Some(&json!({"waiting":true})),
                None,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn paused_cancel_expires_approvals_tools_inputs_and_questions_atomically() {
        let store = Store::connect_encrypted("sqlite::memory:", "test", &[42; 32])
            .await
            .unwrap();
        for status in [TurnStatus::AwaitingApproval, TurnStatus::AwaitingInput] {
            let turn = paused(&store, status).await;
            let other = paused(&store, TurnStatus::AwaitingApproval).await;
            let call = store
                .create_tool_call(
                    ToolRequest {
                        parent_tool_call_id: None,
                        id: Id::new("tool"),
                        scope: scope(),
                        session_id: turn.session_id.clone(),
                        turn_id: turn.id.clone(),
                        tool: "write_file".into(),
                        arguments: json!({}),
                        created_at: Utc::now(),
                    },
                    PolicyResult {
                        decision: PolicyDecision::Ask,
                        policy_id: "test".into(),
                        policy_version: "1".into(),
                        reason: "test".into(),
                        requires_approval: true,
                    },
                    ToolPolicyMetadata::default(),
                    ToolCallStatus::AwaitingApproval,
                )
                .await
                .unwrap();
            let approval = store.create_approval(&call).await.unwrap();
            let request = store
                .create_question_request(
                    &scope(),
                    &QuestionRequest {
                        id: Id::new("question"),
                        session_id: turn.session_id.clone(),
                        turn_id: turn.id.clone(),
                        item_id: Id::new("item"),
                        questions: vec![],
                        allow_other: true,
                        requested_by: scope().actor_id,
                        requested_at: Utc::now(),
                        expires_at: None,
                        status: QuestionStatus::Pending,
                        answers: vec![],
                        answered_by: None,
                        answered_at: None,
                        revision: 1,
                    },
                )
                .await
                .unwrap();
            let input = store
                .create_turn_input(
                    &turn.session_id,
                    CreateTurnInput {
                        scope: scope(),
                        target_turn_id: turn.id.clone(),
                        mode: TurnInputMode::Queue,
                        content: json!("later"),
                        idempotency_key: Id::new("input").0,
                    },
                )
                .await
                .unwrap();
            let cancelled = store.cancel_paused_turn(&scope(), &turn.id).await.unwrap();
            assert_eq!(cancelled.status, TurnStatus::Cancelled);
            assert!(cancelled.checkpoint.is_none());
            assert!(cancelled.completed_at.is_some());
            assert_eq!(
                store.cancel_paused_turn(&scope(), &turn.id).await.unwrap(),
                cancelled
            );
            assert_eq!(
                store.get_approval(&approval.id).await.unwrap().status,
                ApprovalStatus::Expired
            );
            assert_eq!(
                store.get_tool_call(&call.request.id).await.unwrap().status,
                ToolCallStatus::Cancelled
            );
            assert_eq!(
                store
                    .get_question_request(&scope(), &request.id)
                    .await
                    .unwrap()
                    .status,
                QuestionStatus::Cancelled
            );
            let input = store.get_turn_input(&scope(), &input.id).await.unwrap();
            assert_eq!(input.status, TurnInputStatus::Cancelled);
            assert_eq!(input.revision, 2);
            assert!(input.cancelled_at.is_some());
            assert_eq!(
                store.get_turn(&scope(), &other.id).await.unwrap().status,
                TurnStatus::AwaitingApproval
            );
            assert!(matches!(
                store
                    .resolve_approval(&approval.id, &scope(), true, ApprovalScope::Once)
                    .await,
                Err(StorageError::InvalidState(_))
            ));
        }
    }

    #[tokio::test]
    async fn paused_cancel_rejects_other_accounts_and_running_turns() {
        let store = Store::in_memory().await.unwrap();
        let turn = paused(&store, TurnStatus::AwaitingApproval).await;
        let mut other = scope();
        other.actor_id = Id("bob".into());
        assert!(matches!(
            store.cancel_paused_turn(&other, &turn.id).await,
            Err(StorageError::ScopeMismatch)
        ));
        store
            .update_turn(&scope(), &turn.id, TurnStatus::RunningTool, None, None)
            .await
            .unwrap();
        assert!(matches!(
            store.cancel_paused_turn(&scope(), &turn.id).await,
            Err(StorageError::InvalidState(_))
        ));
        assert_eq!(
            store.get_turn(&scope(), &turn.id).await.unwrap().status,
            TurnStatus::RunningTool
        );
        store
            .update_turn(&scope(), &turn.id, TurnStatus::Completed, None, None)
            .await
            .unwrap();
        assert_eq!(
            store
                .cancel_paused_turn(&scope(), &turn.id)
                .await
                .unwrap()
                .status,
            TurnStatus::Completed
        );
    }

    #[tokio::test]
    async fn paused_cancel_and_resume_have_exactly_one_winner() {
        let store = Store::in_memory().await.unwrap();
        for _ in 0..25 {
            let turn = paused(&store, TurnStatus::AwaitingApproval).await;
            let account = scope();
            let (cancel, resume) = tokio::join!(
                store.cancel_paused_turn(&account, &turn.id),
                store.update_turn(&account, &turn.id, TurnStatus::RunningTool, None, None),
            );
            assert_ne!(cancel.is_ok(), resume.is_ok());
            let final_turn = store.get_turn(&account, &turn.id).await.unwrap();
            assert_eq!(
                final_turn.status,
                if cancel.is_ok() {
                    TurnStatus::Cancelled
                } else {
                    TurnStatus::RunningTool
                }
            );
        }
    }
    #[tokio::test]
    async fn paused_cancel_and_approval_claim_are_serialized() {
        let store = Store::in_memory().await.unwrap();
        for approve_first in [false, true] {
            let turn = paused(&store, TurnStatus::AwaitingApproval).await;
            let call = store
                .create_tool_call(
                    ToolRequest {
                        parent_tool_call_id: None,
                        id: Id::new("tool"),
                        scope: scope(),
                        session_id: turn.session_id.clone(),
                        turn_id: turn.id.clone(),
                        tool: "write_file".into(),
                        arguments: json!({}),
                        created_at: Utc::now(),
                    },
                    PolicyResult {
                        decision: PolicyDecision::Ask,
                        policy_id: "test".into(),
                        policy_version: "1".into(),
                        reason: "test".into(),
                        requires_approval: true,
                    },
                    ToolPolicyMetadata::default(),
                    ToolCallStatus::AwaitingApproval,
                )
                .await
                .unwrap();
            let approval = store.create_approval(&call).await.unwrap();
            if approve_first {
                store
                    .resolve_approval(&approval.id, &scope(), true, ApprovalScope::Once)
                    .await
                    .unwrap();
                assert!(matches!(
                    store.cancel_paused_turn(&scope(), &turn.id).await,
                    Err(StorageError::InvalidState(_))
                ));
                assert_eq!(
                    store.get_turn(&scope(), &turn.id).await.unwrap().status,
                    TurnStatus::AwaitingApproval
                );
                assert_eq!(
                    store.get_tool_call(&call.request.id).await.unwrap().status,
                    ToolCallStatus::AwaitingApproval
                );
                assert_eq!(
                    store.get_approval(&approval.id).await.unwrap().status,
                    ApprovalStatus::Approved
                );
            } else {
                store.cancel_paused_turn(&scope(), &turn.id).await.unwrap();
                assert!(matches!(
                    store
                        .resolve_approval(&approval.id, &scope(), true, ApprovalScope::Once)
                        .await,
                    Err(StorageError::InvalidState(_))
                ));
                assert_eq!(
                    store.get_approval(&approval.id).await.unwrap().status,
                    ApprovalStatus::Expired
                );
            }
        }
        // Running operations must use their live executor even with no approval.
        let turn = paused(&store, TurnStatus::AwaitingInput).await;
        store
            .create_tool_call(
                ToolRequest {
                    parent_tool_call_id: None,
                    id: Id::new("tool"),
                    scope: scope(),
                    session_id: turn.session_id.clone(),
                    turn_id: turn.id.clone(),
                    tool: "read_file".into(),
                    arguments: json!({}),
                    created_at: Utc::now(),
                },
                PolicyResult {
                    decision: PolicyDecision::Allow,
                    policy_id: "test".into(),
                    policy_version: "1".into(),
                    reason: "test".into(),
                    requires_approval: false,
                },
                ToolPolicyMetadata::default(),
                ToolCallStatus::Running,
            )
            .await
            .unwrap();
        assert!(matches!(
            store.cancel_paused_turn(&scope(), &turn.id).await,
            Err(StorageError::InvalidState(_))
        ));
    }
}
