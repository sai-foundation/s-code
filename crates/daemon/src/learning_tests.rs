use super::*;
use async_trait::async_trait;
use s_code_agent_core::{AGENT_OP_SCHEMA_VERSION, AgentOp, AgentOperation};
use s_code_model_gateway::{GatewayError, ModelStream};
use s_code_protocol::{PolicyDecision, PolicyResult, ToolRequest};
use s_code_storage::ToolPolicyMetadata;
use tower::ServiceExt;

struct Reply {
    text: String,
    requests: Arc<StdMutex<Vec<ModelRequest>>>,
}

#[async_trait]
impl ModelProvider for Reply {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                text: self.text.clone(),
            }),
            Ok(ModelEvent::Usage {
                input_tokens: 100,
                output_tokens: 50,
            }),
            Ok(ModelEvent::Completed {
                finish_reason: Some("stop".into()),
            }),
        ])))
    }
}

async fn record(
    state: &AppState,
    turn: &Turn,
    name: &str,
    arguments: serde_json::Value,
    result: serde_json::Value,
) -> Id {
    let id = Id::new("tool");
    state
        .store
        .create_tool_call(
            ToolRequest {
                id: id.clone(),
                scope: turn.scope.clone(),
                session_id: turn.session_id.clone(),
                turn_id: turn.id.clone(),
                tool: name.into(),
                arguments,
                created_at: Utc::now(),
            },
            PolicyResult {
                decision: PolicyDecision::Allow,
                policy_id: "test".into(),
                policy_version: "1".into(),
                reason: "mechanism fixture".into(),
                requires_approval: false,
            },
            ToolPolicyMetadata::default(),
            ToolCallStatus::Running,
        )
        .await
        .unwrap();
    state
        .store
        .finish_tool_call(&id, ToolCallStatus::Completed, Some(&result), None)
        .await
        .unwrap();
    id
}

async fn fixture() -> (
    tempfile::TempDir,
    AppState,
    Session,
    Turn,
    serde_json::Value,
) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("clock.py"),
        "def old_clock(value):\n    return 0\n",
    )
    .unwrap();
    let scope = Scope {
        organization_id: Id("org".into()),
        team_id: Id("team".into()),
        actor_id: Id("alice".into()),
        goal_id: None,
        task_id: None,
    };
    let store = Store::in_memory().await.unwrap();
    let session = store
        .create_session(CreateSession {
            scope: scope.clone(),
            workspace_uri: url::Url::from_directory_path(directory.path())
                .unwrap()
                .to_string(),
            title: "Training".into(),
            model: "fixture".into(),
        })
        .await
        .unwrap();
    let state = AppState::new("fixture-auth", store, 0);
    let turn = state.store.create_turn(&scope, &session.id).await.unwrap();
    state
        .store
        .append_turn_message(
            &scope,
            &session.id,
            &turn.id,
            "user",
            serde_json::json!("Fix clock queue_deadline behavior"),
        )
        .await
        .unwrap();
    let read_id = patch(
        &state,
        &session,
        &turn,
        "clock.py",
        "def queue_deadline(value):\n    return value\n",
    )
    .await;
    let verify_id = record(
        &state,
        &turn,
        "run_command",
        serde_json::json!({"program":"python3","args":["-m","unittest","discover"]}),
        serde_json::json!({"exit_code":0,"stdout":"Ran 2 tests\nOK","stderr":""}),
    )
    .await;
    let proposal = serde_json::json!({"lessons":[{"applicability":"queue deadlines and delayed jobs", "guidance":"Use the shared clock.now() for queue deadline calculations, then verify with the queue tests.", "evidence_tool_call_ids":[read_id,verify_id],"dependency_paths":["clock.py"]}]});
    (directory, state, session, turn, proposal)
}

fn result() -> AgentRunResult {
    AgentRunResult {
        status: AgentRunStatus::Completed,
        assistant_text: "Done".into(),
        messages: vec![],
        input_tokens: 10,
        output_tokens: 5,
        model_calls: 1,
        tool_calls: 2,
        unknown_usage_calls: Some(0),
        ops: vec![],
    }
}

fn result_with_ops() -> AgentRunResult {
    let mut result = result();
    result.ops = [
        AgentOperation::UsageAccountingStarted,
        AgentOperation::StatusChanged {
            status: TurnStatus::Completed,
        },
        AgentOperation::TextAppended {
            text: "Done".into(),
        },
        AgentOperation::ModelCallStarted,
        AgentOperation::UsageAdded {
            input_tokens: 10,
            output_tokens: 5,
        },
        AgentOperation::ModelUsageCompleted,
        AgentOperation::ToolCallStarted {
            call_id: "a".into(),
            tool: "apply_patch".into(),
        },
        AgentOperation::ToolCallStarted {
            call_id: "b".into(),
            tool: "run_command".into(),
        },
    ]
    .into_iter()
    .enumerate()
    .map(|(index, operation)| AgentOp {
        schema_version: AGENT_OP_SCHEMA_VERSION,
        sequence: index as u64 + 1,
        operation,
    })
    .collect();
    result
}

fn provider(proposal: serde_json::Value) -> Arc<Reply> {
    Arc::new(Reply {
        text: proposal.to_string(),
        requests: Arc::new(StdMutex::new(Vec::new())),
    })
}

#[tokio::test]
async fn empty_test_runs_and_echoes_are_not_verification() {
    let (_directory, state, session, turn, _) = fixture().await;
    let mut calls = state
        .store
        .list_tool_calls(&turn.scope, &session.id)
        .await
        .unwrap();
    let verify = calls
        .iter_mut()
        .find(|call| call.request.tool == "run_command")
        .unwrap();
    assert!(verification(verify));
    verify.result.as_mut().unwrap()["stdout"] = serde_json::json!("Ran 0 tests\nOK");
    assert!(!verification(verify));
    verify.result.as_mut().unwrap()["stdout"] = serde_json::json!("Ran 2 tests\nOK");
    verify.request.arguments["program"] = serde_json::json!("echo");
    assert!(!verification(verify));
}

#[tokio::test]
async fn learning_controls_require_authentication_and_session_ownership() {
    let (_directory, state, session, _, _) = fixture().await;
    let service = app(state);
    for (token, actor, expected) in [
        (None, "alice", StatusCode::UNAUTHORIZED),
        (Some("fixture-auth"), "bob", StatusCode::FORBIDDEN),
        (Some("fixture-auth"), "alice", StatusCode::OK),
    ] {
        let mut builder = Request::builder().uri(format!(
            "/v1/sessions/{}/learning?organization_id=org&team_id=team&actor_id={actor}",
            session.id.0
        ));
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = service
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}
#[tokio::test]
async fn test_runner_summaries_require_positive_counts() {
    let (_directory, state, session, _, _) = fixture().await;
    let calls = state
        .store
        .list_tool_calls(&session.scope, &session.id)
        .await
        .unwrap();
    let mut call = calls
        .into_iter()
        .find(|c| c.request.tool == "run_command")
        .unwrap();
    for (program, args, output, accepted) in [
        (
            "pytest",
            vec![],
            "========== 10 passed in 0.10s ==========",
            true,
        ),
        (
            "pytest",
            vec![],
            "========== 20 passed in 0.10s ==========",
            true,
        ),
        (
            "cargo",
            vec!["test"],
            "test result: ok. 12 passed; 0 failed\nrunning 0 tests\ntest result: ok. 0 passed; 0 failed",
            true,
        ),
        (
            "python3",
            vec!["-m", "unittest", "-h"],
            "usage: unittest [-h]\nRan 2 tests",
            false,
        ),
        (
            "pytest",
            vec!["--collect-only"],
            "10 tests collected",
            false,
        ),
        ("pytest", vec![], "0 passed", false),
        (
            "python3",
            vec!["-m", "unittest"],
            "usage: unittest [-h]",
            false,
        ),
        ("npm", vec!["test"], "Tests  12 passed (12)", true),
        (
            "go",
            vec!["test", "-run", "^$"],
            "ok\tpkg 0.001s [no tests to run]",
            false,
        ),
        (
            "go",
            vec!["test", "-v"],
            "--- PASS: TestQueue (0.00s)\nPASS\nok\tpkg 0.01s",
            true,
        ),
    ] {
        call.request.arguments = serde_json::json!({"program":program,"args":args});
        call.result = Some(serde_json::json!({"exit_code":0,"stdout":output}));
        assert_eq!(
            verification(&call),
            accepted,
            "{program} {args:?}: {output}"
        );
    }
}

#[tokio::test]
async fn test_fingerprint_detects_opaque_edits_and_removed_verifiers() {
    let (directory, _, session, _, _) = fixture().await;
    let test = directory.path().join("test_clock.py");
    std::fs::write(&test, "assert 1 == 1\n").unwrap();
    let original = verification_fingerprint(&session).unwrap();
    assert!(original.contains_key("test_clock.py"));
    std::fs::write(&test, "pass\n").unwrap();
    assert_ne!(verification_fingerprint(&session).unwrap(), original);
    std::fs::remove_file(&test).unwrap();
    assert_ne!(verification_fingerprint(&session).unwrap(), original);
}

#[test]
fn known_secret_shapes_are_never_eligible_notes() {
    for secret in [
        format!("ghp_{}", "x".repeat(36)),
        format!("AKIA{}", "A".repeat(16)),
        format!("github_pat_{}", "x".repeat(82)),
    ] {
        assert!(!safe_note(&format!("Use {secret} for access"), 1200));
    }
}
#[tokio::test]
async fn outcomes_explain_skips_without_dispatch_or_coding_failure() {
    for (case, expected) in [
        ("docs", ProjectLearningReason::ChangesAfterVerification),
        ("no_verifier", ProjectLearningReason::NoVerifier),
        ("tests", ProjectLearningReason::VerificationChanged),
        ("config", ProjectLearningReason::VerificationChanged),
        ("snapshot", ProjectLearningReason::SnapshotUnavailable),
        ("evidence", ProjectLearningReason::NoReusableObservation),
        ("usage", ProjectLearningReason::UsageIncomplete),
        ("failed_budget", ProjectLearningReason::BudgetExhausted),
        ("failed", ProjectLearningReason::TaskNotCompleted),
        ("cancelled", ProjectLearningReason::Cancelled),
        ("resumed", ProjectLearningReason::ResumedTurn),
    ] {
        let (directory, state, session, mut turn, _) = fixture().await;
        std::fs::write(directory.path().join("test_base.py"), "assert True\n").unwrap();
        std::fs::write(
            directory.path().join("pyproject.toml"),
            "[tool.pytest.ini_options]\n",
        )
        .unwrap();
        state
            .store
            .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Learn)
            .await
            .unwrap();
        let mut guard = begin(&state, &session, &turn).await.unwrap();
        let mut run = result();
        let cancellation = CancellationToken::new();
        match case {
            "docs" => {
                std::fs::write(
                    directory.path().join("README.md"),
                    "Updated documentation\n",
                )
                .unwrap();
                record(
                    &state,
                    &turn,
                    "replace_file",
                    serde_json::json!({"path":"README.md"}),
                    serde_json::json!({"path":"README.md"}),
                )
                .await;
            }
            "no_verifier" => {
                turn = state
                    .store
                    .create_turn(&turn.scope, &session.id)
                    .await
                    .unwrap();
                record(
                    &state,
                    &turn,
                    "read_file",
                    serde_json::json!({"path":"clock.py"}),
                    serde_json::json!({"content":"def now(): return 123"}),
                )
                .await;
            }
            "tests" => std::fs::write(directory.path().join("test_base.py"), "pass\n").unwrap(),
            "config" => std::fs::write(
                directory.path().join("pyproject.toml"),
                "[tool.pytest.ini_options]\ntestpaths=[]\n",
            )
            .unwrap(),
            "snapshot" => guard.original = None,
            "evidence" => {
                std::fs::write(directory.path().join("clock.py"), "def now(): return 999\n")
                    .unwrap()
            }
            "usage" => run.unknown_usage_calls = None,
            "budget" => run.model_calls = TurnLimits::default().max_model_calls,
            "failed_budget" => {
                run.status = AgentRunStatus::Failed {
                    reason: s_code_agent_core::MODEL_CALL_LIMIT_REASON.into(),
                }
            }
            "failed" => {
                run.status = AgentRunStatus::Failed {
                    reason: "private provider detail must not enter outcome".into(),
                }
            }
            "cancelled" => cancellation.cancel(),
            "resumed" => guard.resumed = true,
            _ => unreachable!(),
        }
        let original_status = run.status.clone();
        let original_calls = run.model_calls;
        finish(&state, &session, &turn, &run, &cancellation, guard).await;
        assert_eq!(run.status, original_status, "{case}");
        assert_eq!(run.model_calls, original_calls, "{case}");
        let outcome = state
            .store
            .project_learning_settings(&turn.scope, &session.workspace_uri)
            .await
            .unwrap()
            .last_outcome
            .unwrap();
        assert_eq!(
            outcome.status,
            if case == "evidence" {
                ProjectLearningStatus::Empty
            } else {
                ProjectLearningStatus::Skipped
            },
            "{case}"
        );
        assert_eq!(outcome.reason, expected, "{case}");
        assert_eq!(outcome.saved_count, 0);
        assert_eq!(outcome.source_turn_id, turn.id);
        assert!(
            !serde_json::to_string(&outcome)
                .unwrap()
                .contains("private provider")
        );
    }
}

async fn enable(state: &AppState, session: &Session, turn: &Turn) -> LearningGuard {
    state
        .store
        .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Learn)
        .await
        .unwrap();
    begin(state, session, turn).await.unwrap()
}

async fn verify(state: &AppState, turn: &Turn) -> Id {
    record(
        state,
        turn,
        "run_command",
        serde_json::json!({"program":"python3","args":["-m","unittest","discover"]}),
        serde_json::json!({"exit_code":0,"stdout":"Ran 2 tests\nOK","stderr":""}),
    )
    .await
}

async fn read(
    state: &AppState,
    session: &Session,
    turn: &Turn,
    path: &str,
    start: usize,
    end: usize,
    cap: usize,
) -> Id {
    let value = runtime(session)
        .unwrap()
        .read_file(path, start, end, cap)
        .unwrap();
    record(
        state,
        turn,
        "read_file",
        serde_json::json!({"path":path}),
        serde_json::to_value(value).unwrap(),
    )
    .await
}

// Produce actual execution results rather than duplicate change-summary logic.
async fn patch(state: &AppState, session: &Session, turn: &Turn, path: &str, text: &str) -> Id {
    let before = runtime(session).unwrap().snapshot_file(path).unwrap();
    let mut outcome = state.execution.submit_for_turn(&session.id, &turn.id, SubmitToolCall {
        scope: turn.scope.clone(), tool: "apply_patch".into(),
        arguments: serde_json::json!({"path":path,"content":text,"expected_revision":before.sha256}),
    }).await.unwrap();
    if let ToolCallOutcome::AwaitingApproval { approval, .. } = outcome {
        outcome = state
            .execution
            .resolve(
                &approval.id,
                ResolveApproval {
                    scope: turn.scope.clone(),
                    approved: true,
                    approval_scope: s_code_protocol::ApprovalScope::Once,
                },
            )
            .await
            .unwrap();
    }
    let ToolCallOutcome::Completed { tool_call } = outcome else {
        panic!("patch must complete: {outcome:?}")
    };
    tool_call.request.id
}

async fn stored(state: &AppState, session: &Session) -> Vec<ProjectLesson> {
    state
        .store
        .list_project_lessons(&session.scope, &session.workspace_uri)
        .await
        .unwrap()
}

async fn outcome(state: &AppState, session: &Session) -> ProjectLearningOutcome {
    state
        .store
        .project_learning_settings(&session.scope, &session.workspace_uri)
        .await
        .unwrap()
        .last_outcome
        .unwrap()
}

#[tokio::test]
async fn local_collection_preserves_coding_usage_checkpoint_and_emits_content_free_completion() {
    let (_directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    let run = result_with_ops();
    let before = serde_json::to_value(AgentCheckpoint::new(run.clone())).unwrap();
    finish(
        &state,
        &session,
        &turn,
        &run,
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert_eq!(
        serde_json::to_value(AgentCheckpoint::new(run)).unwrap(),
        before
    );
    AgentCheckpoint::decode(before).unwrap();
    let items = stored(&state, &session).await;
    assert_eq!(items.len(), 1);
    let source = items[0].source_observation.as_ref().unwrap();
    assert_eq!(source.path, "clock.py");
    assert_eq!(
        source.fragments[0].text,
        "def queue_deadline(value):\n    return value\n"
    );
    assert!(!serde_json::to_string(&items).unwrap().contains("Fix clock"));
    let last = outcome(&state, &session).await;
    assert_eq!(
        (last.status, last.reason, last.saved_count),
        (
            ProjectLearningStatus::Saved,
            ProjectLearningReason::Saved,
            1
        )
    );
    let events = state
        .store
        .list_events(&turn.scope.team_id, 0, 100)
        .await
        .unwrap();
    let event = events
        .iter()
        .find(|event| event.kind == "learning.completed")
        .unwrap();
    assert_eq!(event.payload["mechanism"], "verified_source_change");
    assert_eq!(event.payload["model_calls"], 0);
    assert_eq!(event.payload["coding_usage_complete"], true);
    assert!(!event.payload.to_string().contains("queue_deadline"));
}

#[tokio::test]
async fn completed_at_call_budget_needs_no_reserve_for_local_collection() {
    let (_directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    let mut run = result();
    run.model_calls = TurnLimits::default().max_model_calls;
    finish(
        &state,
        &session,
        &turn,
        &run,
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert_eq!(stored(&state, &session).await.len(), 1);
    assert_eq!(run.model_calls, TurnLimits::default().max_model_calls);
}

#[tokio::test]
async fn verification_after_documentation_edits_restores_eligibility_but_overlap_is_rejected() {
    let (directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    std::fs::write(directory.path().join("README.md"), "Clock documentation\n").unwrap();
    let edit = record(
        &state,
        &turn,
        "replace_file",
        serde_json::json!({"path":"README.md"}),
        serde_json::json!({"path":"README.md"}),
    )
    .await;
    verify(&state, &turn).await;
    // This simulates an earlier-created mutation finishing after verification began.
    state
        .store
        .finish_tool_call(
            &edit,
            ToolCallStatus::Completed,
            Some(&serde_json::json!({"path":"README.md"})),
            None,
        )
        .await
        .unwrap();
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert_eq!(
        outcome(&state, &session).await.reason,
        ProjectLearningReason::ChangesAfterVerification
    );
    assert!(stored(&state, &session).await.is_empty());
    let guard = begin(&state, &session, &turn).await.unwrap();
    verify(&state, &turn).await;
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert_eq!(stored(&state, &session).await.len(), 1);
}

#[tokio::test]
async fn only_preverification_writes_of_the_current_version_are_saved_without_rereading() {
    let (directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    let text = "def queue_deadline(value):\n    return value + 1\n";
    std::fs::write(directory.path().join("clock.py"), text).unwrap();
    read(&state, &session, &turn, "clock.py", 1, 10, 1000).await;
    verify(&state, &turn).await;
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert!(stored(&state, &session).await.is_empty());
    assert_eq!(
        outcome(&state, &session).await.reason,
        ProjectLearningReason::NoReusableObservation
    );
    let guard = begin(&state, &session, &turn).await.unwrap();
    let edit = patch(
        &state,
        &session,
        &turn,
        "clock.py",
        "def queue_deadline(value):\n    return value + 2\n",
    )
    .await;
    let last = verify(&state, &turn).await;
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    let saved = stored(&state, &session).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].evidence_tool_call_ids, vec![edit, last]);
    assert!(saved[0].id.0.starts_with("change_"));
    let observation = saved[0].source_observation.as_ref().unwrap();
    assert!(
        observation
            .change
            .as_ref()
            .unwrap()
            .previous_sha256
            .is_some()
    );
    assert_eq!((observation.start_line, observation.end_line), (2, 2));
    assert_eq!(observation.fragments[0].text, "    return value + 2\n");
}

#[tokio::test]
async fn cancel_clear_and_mode_cycle_during_collection_cannot_save_late_source_context() {
    for action in ["cancel", "clear", "off", "reuse", "file", "tests"] {
        let (directory, state, session, turn, _) = fixture().await;
        std::fs::write(directory.path().join("test_base.py"), "assert True\n").unwrap();
        let mut guard = enable(&state, &session, &turn).await;
        let ready = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        guard.pause_before_save = Some((ready.clone(), resume.clone()));
        let cancellation = CancellationToken::new();
        let task = tokio::spawn({
            let state = state.clone();
            let session = session.clone();
            let turn = turn.clone();
            let cancellation = cancellation.clone();
            async move { finish(&state, &session, &turn, &result(), &cancellation, guard).await }
        });
        ready.notified().await;
        match action {
            "cancel" => cancellation.cancel(),
            "clear" => {
                state
                    .store
                    .revoke_project_lessons(&turn.scope, &session.workspace_uri, None)
                    .await
                    .unwrap();
            }
            "off" | "reuse" => {
                state
                    .store
                    .set_project_learning(
                        &turn.scope,
                        &session.workspace_uri,
                        if action == "off" {
                            LearningMode::Off
                        } else {
                            LearningMode::Reuse
                        },
                    )
                    .await
                    .unwrap();
                state
                    .store
                    .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Learn)
                    .await
                    .unwrap();
            }
            "file" => std::fs::write(directory.path().join("clock.py"), "changed\n").unwrap(),
            "tests" => std::fs::write(directory.path().join("test_base.py"), "pass\n").unwrap(),
            _ => unreachable!(),
        }
        resume.notify_one();
        task.await.unwrap();
        assert!(stored(&state, &session).await.is_empty(), "{action}");
        let settings = state
            .store
            .project_learning_settings(&turn.scope, &session.workspace_uri)
            .await
            .unwrap();
        if matches!(action, "clear" | "off" | "reuse") {
            assert!(settings.last_outcome.is_none(), "{action}");
            assert!(
                !state
                    .store
                    .list_events(&turn.scope.team_id, 0, 100)
                    .await
                    .unwrap()
                    .iter()
                    .any(|event| event.kind == "learning.completed")
            );
        } else {
            assert_eq!(
                settings.last_outcome.unwrap().reason,
                match action {
                    "cancel" => ProjectLearningReason::Cancelled,
                    "tests" => ProjectLearningReason::VerificationChanged,
                    _ => ProjectLearningReason::EvidenceUnavailable,
                }
            );
        }
    }
}

#[tokio::test]
async fn fragments_preserve_true_lines_utf8_crlf_and_reject_secrets_before_clipping() {
    let (directory, state, session, turn, _) = fixture().await;
    let text = (1..=100)
        .map(|line| format!("# queue_deadline 行{line:03} {}\r\n", "中".repeat(8)))
        .collect::<String>();
    let before = text
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, line)| {
            if (10..90).contains(&index) {
                "old\r\n"
            } else {
                line
            }
        })
        .collect::<String>();
    std::fs::write(directory.path().join("clock.py"), before).unwrap();
    let edit_id = patch(&state, &session, &turn, "clock.py", &text).await;
    let verify_id = verify(&state, &turn).await;
    let call = state.store.get_tool_call(&edit_id).await.unwrap();
    let verifier = state.store.get_tool_call(&verify_id).await.unwrap();
    assert_eq!(
        call.result.as_ref().unwrap()["change_summary"]["after_span"]["truncated"],
        true
    );
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    let lesson =
        observation_from_change(&call, &verifier, &text, &hash, &turn, Utc::now()).unwrap();
    assert!(serde_json::to_vec(&lesson).unwrap().len() <= MAX_SOURCE_RECORD_BYTES);
    let observation = lesson.source_observation.unwrap();
    assert_eq!(
        (
            observation.start_line,
            observation.end_line,
            observation.fragments.len()
        ),
        (11, 90, 2)
    );
    assert!(observation.truncated);
    for fragment in observation.fragments {
        let lines = fragment.text.split_inclusive('\n').count();
        assert_eq!(
            fragment.text,
            text.split_inclusive('\n')
                .skip(fragment.start_line as usize - 1)
                .take(lines)
                .collect::<String>()
        );
        assert!(fragment.text.ends_with("\r\n"));
    }
    // Secrets outside retained fragments, including outside the changed range,
    // must not become safe because previewing or cropping omitted their marker.
    let secret = format!("{}{}", "sk-", "x".repeat(45));
    for line in ["行001", "行050"] {
        let sensitive = text.replacen(line, &secret, 1);
        let hash = format!("{:x}", Sha256::digest(sensitive.as_bytes()));
        let mut candidate = call.clone();
        candidate.result.as_mut().unwrap()["sha256"] = serde_json::json!(hash);
        assert!(
            observation_from_change(&candidate, &verifier, &sensitive, &hash, &turn, Utc::now())
                .is_none()
        );
    }
    let pem = format!(
        "-----BEGIN {}-----\n{}\n-----END {}-----\n",
        "PRIVATE KEY",
        "a".repeat(3000),
        "PRIVATE KEY"
    );
    assert!(!safe_source(&pem));
}

#[tokio::test]
async fn preview_truncation_does_not_truncate_the_verified_current_span_or_final_line() {
    let (_directory, state, session, turn, _) = fixture().await;
    let text = "queue_deadline one\r\nqueue_deadline 中文字\r\nqueue_deadline final";
    let id = patch(&state, &session, &turn, "clock.py", text).await;
    let v = verify(&state, &turn).await;
    let mut call = state.store.get_tool_call(&id).await.unwrap();
    let verifier = state.store.get_tool_call(&v).await.unwrap();
    // Model-facing previews may end partway through a logical line. The full
    // current source, hash and complete span still carry the entire final line.
    call.result.as_mut().unwrap()["change_summary"]["after_span"]["excerpt"] =
        serde_json::json!(&text[..7]);
    call.result.as_mut().unwrap()["change_summary"]["after_span"]["truncated"] =
        serde_json::json!(true);
    let lesson = observation_from_change(
        &call,
        &verifier,
        text,
        &format!("{:x}", Sha256::digest(text.as_bytes())),
        &turn,
        Utc::now(),
    )
    .unwrap();
    let source = lesson.source_observation.unwrap();
    assert!(!source.truncated);
    assert_eq!(
        source.fragments,
        vec![SourceFragment {
            start_line: 1,
            text: text.into()
        }]
    );
}

#[tokio::test]
async fn ordinary_task_paths_survive_but_secret_paths_and_metadata_do_not() {
    let (_directory, state, _session, turn, _) = fixture().await;
    let calls = state
        .store
        .learning_tool_calls(&turn.scope, &turn.id)
        .await
        .unwrap();
    let original = calls
        .iter()
        .find(|call| call.request.tool == "apply_patch")
        .unwrap();
    let verifier = calls
        .iter()
        .find(|call| call.request.tool == "run_command")
        .unwrap();
    let text = "def queue_deadline(value):\n    return value\n";
    let hash = original.result.as_ref().unwrap()["sha256"]
        .as_str()
        .unwrap();
    for path in [
        "task-1.py".to_string(),
        "task-{number}.py".into(),
        "risk-based/task-long-normal-ID.py".into(),
        format!("{}.py", ["sk-", &"x".repeat(36)].concat()),
        "clock\nfile.py".into(),
    ] {
        let mut call = original.clone();
        call.request.arguments["path"] = serde_json::json!(path);
        call.result.as_mut().unwrap()["path"] = serde_json::json!(path);
        let expected = !path.contains(&["sk-", "xxx"].concat()) && !path.contains('\n');
        assert_eq!(
            observation_from_change(&call, verifier, text, hash, &turn, Utc::now()).is_some(),
            expected,
            "path category"
        );
    }
    let mut call = original.clone();
    call.request.id = Id(["ghp_", &"x".repeat(36)].concat());
    assert!(observation_from_change(&call, verifier, text, hash, &turn, Utc::now()).is_none());
}

#[tokio::test]
async fn candidates_are_file_distinct_relevant_and_deterministic_with_bounded_payloads() {
    let (_directory, state, session, turn, _) = fixture().await;
    // Recent unrelated writes cannot evict task-relevant source. Ties choose
    // paths lexically; read observations cannot add another candidate slot.
    for path in ["zeta.py", "beta.py", "alpha.py", "unrelated.py"] {
        patch(
            &state,
            &session,
            &turn,
            path,
            if path == "unrelated.py" {
                "unrelated information\n"
            } else {
                "queue_deadline shared helper\n"
            },
        )
        .await;
    }
    read(&state, &session, &turn, "alpha.py", 1, 1, 1000).await;
    verify(&state, &turn).await;
    let items = collect(&state, &session, &turn).await.unwrap().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item.files[0].path.as_str())
            .collect::<Vec<_>>(),
        vec!["clock.py", "alpha.py", "beta.py"]
    );
    assert!(
        items
            .iter()
            .all(|item| serde_json::to_vec(item).unwrap().len() <= 3_200)
    );
    assert!(terms("Please implement a new function in this project").is_empty());
}

#[tokio::test]
async fn oversized_evidence_is_skipped_without_failing_coding() {
    let (_directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    record(
        &state,
        &turn,
        "read_file",
        serde_json::json!({"path":"large.py"}),
        serde_json::json!({"content":"x".repeat(MAX_SOURCE_PROCESSING_BYTES + 1)}),
    )
    .await;
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    assert_eq!(
        outcome(&state, &session).await.reason,
        ProjectLearningReason::EvidenceUnavailable
    );
    assert!(stored(&state, &session).await.is_empty());
}
#[tokio::test]
async fn recall_revalidates_every_request_without_persisting_notes_in_checkpoints() {
    let (_directory, state, session, turn, _) = fixture().await;
    state
        .store
        .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Learn)
        .await
        .unwrap();
    let guard = begin(&state, &session, &turn).await.unwrap();
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    state
        .store
        .update_turn(&turn.scope, &turn.id, TurnStatus::Completed, None, None)
        .await
        .unwrap();
    let reply = provider(serde_json::json!({"lessons":[]}));
    let wrapped = with_experience(
        state.clone(),
        session.clone(),
        "clock queue_deadline".into(),
        reply.clone(),
    );
    let request = ModelRequest {
        reasoning_effort: None,
        model: "fixture".into(),
        temperature: 0.0,
        messages: vec![ModelMessage {
            role: "user".into(),
            content: serde_json::json!("clock queue_deadline"),
        }],
        tools: vec![],
        max_output_tokens: 100,
        routing: Default::default(),
    };
    drop(wrapped.stream(request.clone()).await.unwrap());
    assert_eq!(reply.requests.lock().unwrap()[0].messages.len(), 2);
    assert_eq!(request.messages.len(), 1);
    let old = reply.requests.lock().unwrap()[0].clone();
    // Even a legacy resumed checkpoint containing an old note is sanitized.
    state
        .store
        .revoke_project_lessons(&turn.scope, &session.workspace_uri, None)
        .await
        .unwrap();
    drop(wrapped.stream(old).await.unwrap());
    assert_eq!(reply.requests.lock().unwrap()[1].messages.len(), 1);
    state
        .store
        .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Off)
        .await
        .unwrap();
    drop(wrapped.stream(request).await.unwrap());
    assert_eq!(reply.requests.lock().unwrap()[2].messages.len(), 1);
}

struct IncrementalReply {
    text: String,
}
#[async_trait]
impl ModelProvider for IncrementalReply {
    async fn stream(&self, _: ModelRequest) -> Result<ModelStream, GatewayError> {
        Ok(Box::pin(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                text: self.text.clone(),
            }),
            Ok(ModelEvent::Usage {
                input_tokens: 13,
                output_tokens: 0,
            }),
            Ok(ModelEvent::Usage {
                input_tokens: 0,
                output_tokens: 5,
            }),
            Ok(ModelEvent::Completed {
                finish_reason: Some("STOP".into()),
            }),
        ])))
    }
}

#[tokio::test]
async fn incremental_provider_usage_and_uppercase_stop_preserve_checkpoint_projection() {
    let (_directory, state, session, turn, _) = fixture().await;
    enable(&state, &session, &turn).await;
    execute_turn(
        state.clone(),
        Arc::new(IncrementalReply {
            text: "Done".into(),
        }),
        turn.clone(),
        CancellationToken::new(),
        vec![ModelMessage {
            role: "user".into(),
            content: serde_json::json!("clock queue_deadline"),
        }],
        TurnExecutionOptions {
            profile: ToolProfile::Default,
            step_inputs: None,
            generate_title: false,
        },
    )
    .await
    .unwrap();
    let completed = state.store.get_turn(&turn.scope, &turn.id).await.unwrap();
    assert_eq!(completed.status, TurnStatus::Completed);
    let checkpoint = AgentCheckpoint::decode(completed.checkpoint.unwrap()).unwrap();
    assert_eq!(
        (
            checkpoint.result.input_tokens,
            checkpoint.result.output_tokens,
            checkpoint.result.model_calls
        ),
        (13, 5, 1)
    );
    assert_eq!(checkpoint.result.unknown_usage_calls, Some(0));
    assert!(
        !checkpoint
            .result
            .messages
            .iter()
            .any(|message| message.content["type"] == "untrusted_project_experience")
    );
    assert_eq!(stored(&state, &session).await.len(), 1);
}

#[tokio::test]
async fn observations_transfer_only_to_same_actor_and_project_and_legacy_is_view_only() {
    let (directory, state, session, turn, _) = fixture().await;
    let guard = enable(&state, &session, &turn).await;
    finish(
        &state,
        &session,
        &turn,
        &result(),
        &CancellationToken::new(),
        guard,
    )
    .await;
    state
        .store
        .update_turn(&turn.scope, &turn.id, TurnStatus::Completed, None, None)
        .await
        .unwrap();
    let other_directory = tempfile::tempdir().unwrap();
    for (actor, workspace, expected) in [
        ("alice", session.workspace_uri.clone(), true),
        ("bob", session.workspace_uri.clone(), false),
        (
            "alice",
            url::Url::from_directory_path(other_directory.path())
                .unwrap()
                .to_string(),
            false,
        ),
    ] {
        let other = state
            .store
            .create_session(CreateSession {
                scope: Scope {
                    actor_id: Id(actor.into()),
                    ..session.scope.clone()
                },
                workspace_uri: workspace,
                title: "Next task".into(),
                model: session.model.clone(),
            })
            .await
            .unwrap();
        state
            .store
            .set_project_learning(&other.scope, &other.workspace_uri, LearningMode::Reuse)
            .await
            .unwrap();
        let context = retrieve(&state, &other, "clock queue_deadline")
            .await
            .unwrap();
        assert_eq!(context.is_some(), expected);
        if let Some(context) = context {
            assert!(context.content.get("source_observations").is_some());
            assert!(context.content.get("lessons").is_none());
            assert!(estimate_tokens(&context.content.to_string()) <= MAX_RETRIEVAL_TOKENS);
        }
    }
    let current = stored(&state, &session).await.remove(0);
    let mut old_read = current.clone();
    old_read.id = Id::new("old-read");
    old_read.source_observation.as_mut().unwrap().change = None;
    let mut legacy = current.clone();
    legacy.source_observation = None;
    legacy.id = Id::new("legacy");
    legacy.applicability = "clock queue_deadline".into();
    legacy.guidance = "Old generated guidance".into();
    let settings = state
        .store
        .set_project_learning(&turn.scope, &session.workspace_uri, LearningMode::Learn)
        .await
        .unwrap();
    state
        .store
        .save_project_lessons(
            &turn.scope,
            &session.workspace_uri,
            settings.generation,
            &[legacy, old_read],
        )
        .await
        .unwrap();
    assert_eq!(stored(&state, &session).await.len(), 3);
    state
        .store
        .revoke_project_lessons(&turn.scope, &session.workspace_uri, Some(&current.id))
        .await
        .unwrap();
    assert!(
        retrieve(&state, &session, "clock queue_deadline")
            .await
            .unwrap()
            .is_none()
    );
    std::fs::write(directory.path().join("clock.py"), "updated clock\n").unwrap();
    assert!(
        retrieve(&state, &session, "clock queue_deadline")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(stored(&state, &session).await.len(), 2); // Recall never mutates storage.
}

#[tokio::test]
async fn malformed_change_metadata_cannot_become_source_evidence() {
    let (_directory, state, _session, turn, _) = fixture().await;
    let calls = state
        .store
        .learning_tool_calls(&turn.scope, &turn.id)
        .await
        .unwrap();
    let original = calls
        .iter()
        .find(|call| call.request.tool == "apply_patch")
        .unwrap();
    let verifier = calls
        .iter()
        .find(|call| call.request.tool == "run_command")
        .unwrap();
    let full = "def queue_deadline(value):\n    return value\n";
    let hash = format!("{:x}", Sha256::digest(full.as_bytes()));
    assert!(observation_from_change(original, verifier, full, &hash, &turn, Utc::now()).is_some());
    for (pointer, value) in [
        ("/bytes_written", serde_json::json!(1)),
        ("/sha256", serde_json::json!("a".repeat(64))),
        ("/previous_sha256", serde_json::json!(hash)),
        ("/previous_sha256", serde_json::json!("not-a-hash")),
        ("/path", serde_json::json!("different.py")),
        ("/change_summary/first_changed_line", serde_json::json!(0)),
        (
            "/change_summary/first_changed_line",
            serde_json::json!(u64::MAX),
        ),
        ("/change_summary/after_total_lines", serde_json::json!(3)),
        ("/change_summary/before_total_lines", serde_json::json!(3)),
        (
            "/change_summary/after_span/line_count",
            serde_json::json!(1),
        ),
        ("/change_summary/after_span/bytes", serde_json::json!(1)),
        (
            "/change_summary/after_span/excerpt",
            serde_json::json!("wrong source"),
        ),
        (
            "/change_summary/after_span/truncated",
            serde_json::json!(true),
        ),
        ("/change_summary/before_span/bytes", serde_json::json!(999)),
        (
            "/change_summary/before_span/line_count",
            serde_json::json!(u64::MAX),
        ),
    ] {
        let mut call = original.clone();
        *call.result.as_mut().unwrap().pointer_mut(pointer).unwrap() = value;
        assert!(
            observation_from_change(&call, verifier, full, &hash, &turn, Utc::now()).is_none(),
            "{pointer}"
        );
    }
    for side in ["before_span", "after_span"] {
        let mut call = original.clone();
        call.result.as_mut().unwrap()["change_summary"][side]["redacted"] = serde_json::json!(true);
        assert!(observation_from_change(&call, verifier, full, &hash, &turn, Utc::now()).is_none());
    }
    for missing in ["previous_sha256", "change_summary", "bytes_written"] {
        let mut call = original.clone();
        call.result
            .as_mut()
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(missing);
        assert!(observation_from_change(&call, verifier, full, &hash, &turn, Utc::now()).is_none());
    }
    let mut failed = original.clone();
    failed.status = ToolCallStatus::Failed;
    assert!(observation_from_change(&failed, verifier, full, &hash, &turn, Utc::now()).is_none());
}

#[tokio::test]
async fn new_files_and_enclosing_spans_are_supported_but_noops_and_deletions_are_not() {
    let (_directory, state, session, turn, _) = fixture().await;
    let text = "queue_deadline first\nunchanged middle\nqueue_deadline last\n";
    let created = patch(&state, &session, &turn, "added.py", text).await;
    let verification_id = verify(&state, &turn).await;
    let call = state.store.get_tool_call(&created).await.unwrap();
    let verifier = state.store.get_tool_call(&verification_id).await.unwrap();
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    let lesson = observation_from_change(&call, &verifier, text, &hash, &turn, Utc::now()).unwrap();
    assert_eq!(
        lesson.source_observation.unwrap().change,
        Some(SourceChangeEvidence {
            previous_sha256: None
        })
    );
    let noop = patch(&state, &session, &turn, "added.py", text).await;
    let deletion_text = "unchanged middle\nqueue_deadline last\n";
    let deletion = patch(&state, &session, &turn, "added.py", deletion_text).await;
    let verification_id = verify(&state, &turn).await;
    let verifier = state.store.get_tool_call(&verification_id).await.unwrap();
    for (id, full) in [(noop, text), (deletion, deletion_text)] {
        let call = state.store.get_tool_call(&id).await.unwrap();
        let hash = format!("{:x}", Sha256::digest(full.as_bytes()));
        assert!(
            observation_from_change(&call, &verifier, full, &hash, &turn, Utc::now()).is_none()
        );
    }
    let final_text = "queue_deadline changed\nunchanged middle\nqueue_deadline changed last\n";
    patch(&state, &session, &turn, "added.py", text).await;
    let edit = patch(&state, &session, &turn, "added.py", final_text).await;
    let verification_id = verify(&state, &turn).await;
    let call = state.store.get_tool_call(&edit).await.unwrap();
    let verifier = state.store.get_tool_call(&verification_id).await.unwrap();
    let hash = format!("{:x}", Sha256::digest(final_text.as_bytes()));
    let lesson =
        observation_from_change(&call, &verifier, final_text, &hash, &turn, Utc::now()).unwrap();
    let source = lesson.source_observation.unwrap();
    assert_eq!((source.start_line, source.end_line), (1, 3));
    assert_eq!(source.fragments[0].text, final_text); // Contains the unchanged middle by design.
    assert!(!source.truncated);
}

#[tokio::test]
async fn nonterminal_mutators_before_verification_are_not_assumed_finished() {
    for status in [
        ToolCallStatus::Proposed,
        ToolCallStatus::AwaitingApproval,
        ToolCallStatus::Running,
        ToolCallStatus::Completed,
        ToolCallStatus::Failed,
        ToolCallStatus::Denied,
        ToolCallStatus::Cancelled,
    ] {
        let (_directory, state, session, turn, _) = fixture().await;
        let id = record(
            &state,
            &turn,
            "replace_file",
            serde_json::json!({"path":"other.py"}),
            serde_json::json!({"path":"other.py"}),
        )
        .await;
        state
            .store
            .finish_tool_call(&id, status.clone(), None, None)
            .await
            .unwrap();
        let verification_id = verify(&state, &turn).await;
        let mutation = state.store.get_tool_call(&id).await.unwrap();
        let verifier = state.store.get_tool_call(&verification_id).await.unwrap();
        assert!(mutation.created_at < verifier.created_at);
        assert!(mutation.updated_at <= verifier.created_at);
        let collected = collect(&state, &session, &turn).await.unwrap();
        if matches!(
            status,
            ToolCallStatus::Proposed | ToolCallStatus::AwaitingApproval | ToolCallStatus::Running
        ) {
            assert_eq!(
                collected.unwrap_err(),
                ProjectLearningReason::ChangesAfterVerification
            );
        } else {
            assert_eq!(
                collected.unwrap().len(),
                1,
                "terminal {status:?} before verification remains valid"
            );
        }
    }
}
