//! Optional, read-only JavaScript orchestration. The VM lives in a disposable process;
//! only the host can authorize tools or publish their execution events.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::{BufRead, Write},
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail, ensure};
use rquickjs::{Context, Function, Object, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "search_text",
    "git_status",
    "git_diff",
];
pub(crate) const MAX_CALLS: usize = 32;
pub(crate) const MAX_CODE: usize = 32 * 1024;
pub(crate) const MAX_FRAME: usize = 1024 * 1024;
pub(crate) const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 32 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkerMessage {
    Call {
        id: u32,
        tool: String,
        arguments: Value,
    },
    Done {
        output: Vec<Value>,
    },
    Failed {
        error: String,
    },
}

fn read_frame(reader: &mut impl BufRead) -> Result<String> {
    let mut bytes = Vec::new();
    std::io::Read::take(reader, (MAX_FRAME + 1) as u64).read_until(b'\n', &mut bytes)?;
    ensure!(!bytes.is_empty(), "Code Mode host disconnected");
    ensure!(bytes.len() <= MAX_FRAME, "Code Mode message too large");
    Ok(String::from_utf8(bytes)?)
}

fn send(message: &WorkerMessage) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, message)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

/// Private stdio entry point. No standard-library bindings, module loader, host
/// environment, filesystem, network, or shell are exposed to JavaScript.
pub fn worker() -> Result<()> {
    if let Err(error) = worker_inner() {
        send(&WorkerMessage::Failed {
            error: format!("{error:#}"),
        })?;
    }
    Ok(())
}

fn worker_inner() -> Result<()> {
    let mut stdin = std::io::stdin().lock();
    let input: Value = serde_json::from_str(&read_frame(&mut stdin)?)?;
    let code = input["code"].as_str().context("code must be a string")?;
    ensure!(code.len() <= MAX_CODE, "Code Mode source exceeds 32 KiB");
    let runtime = Runtime::new()?;
    runtime.set_memory_limit(64 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    let deadline = Instant::now() + TIMEOUT;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime)?;
    let requests = Rc::new(RefCell::new(VecDeque::new()));
    let output = Rc::new(RefCell::new(Vec::<Value>::new()));
    let completion = Rc::new(RefCell::new(None::<Option<String>>));
    context.with(|ctx| -> Result<()> {
        let requests_ref = requests.clone();
        let call_count = Cell::new(0);
        let send_call = Function::new(
            ctx.clone(),
            move |tool: String, args: String| -> rquickjs::Result<u32> {
                if call_count.get() >= MAX_CALLS
                    || args.len() > 64 * 1024
                    || !TOOLS.contains(&tool.as_str())
                {
                    return Err(rquickjs::Error::Unknown);
                }
                let arguments: Value =
                    serde_json::from_str(&args).map_err(|_| rquickjs::Error::Unknown)?;
                if !arguments.is_object() {
                    return Err(rquickjs::Error::Unknown);
                }
                call_count.set(call_count.get() + 1);
                let id = call_count.get() as u32;
                requests_ref.borrow_mut().push_back(WorkerMessage::Call {
                    id,
                    tool,
                    arguments,
                });
                Ok(id)
            },
        )?;
        let output_ref = output.clone();
        let output_bytes = Cell::new(0);
        let emit = Function::new(
            ctx.clone(),
            move |encoded: String| -> rquickjs::Result<()> {
                output_bytes.set(output_bytes.get() + encoded.len());
                if output_bytes.get() > MAX_OUTPUT || output_ref.borrow().len() >= 128 {
                    return Err(rquickjs::Error::Unknown);
                }
                output_ref
                    .borrow_mut()
                    .push(serde_json::from_str(&encoded).map_err(|_| rquickjs::Error::Unknown)?);
                Ok(())
            },
        )?;
        let completion_ref = completion.clone();
        let finish = Function::new(ctx.clone(), move |error: Option<String>| {
            *completion_ref.borrow_mut() =
                Some(error.map(|value| value.chars().take(1024).collect()));
        })?;
        let bootstrap: Function = ctx.eval(include_str!("code_mode_bootstrap.js"))?;
        let bridge: Object = bootstrap.call((send_call, emit, finish, TOOLS.join(",")))?;
        let start: Function = bridge.get("start")?;
        let deliver: Function = bridge.get("deliver")?;
        let pending: Function = bridge.get("pending")?;
        let program: Function = ctx
            .eval(format!("(async function() {{\n{code}\n}})"))
            .map_err(|_| anyhow::anyhow!("Invalid JavaScript program"))?;
        start.call::<_, ()>((program,))?;
        loop {
            while ctx.execute_pending_job() {
                ensure!(
                    Instant::now() < deadline,
                    "JavaScript exceeded its time limit"
                );
            }
            if let Some(error) = completion.borrow().as_ref() {
                if let Some(error) = error {
                    bail!("JavaScript failed: {error}");
                }
                ensure!(
                    pending.call::<_, usize>(())? == 0,
                    "Await all tool calls before the program finishes"
                );
                send(&WorkerMessage::Done {
                    output: output.borrow().clone(),
                })?;
                return Ok(());
            }
            for request in requests.borrow_mut().drain(..) {
                send(&request)?;
            }
            ensure!(
                pending.call::<_, usize>(())? > 0,
                "Program is waiting on a promise with no tool call to complete"
            );
            let response = read_frame(&mut stdin)?;
            deliver.call::<_, ()>((response,))?;
        }
    })
}

use crate::DaemonToolExecutor;
use futures_util::{StreamExt, stream::FuturesUnordered};
use s_code_agent_core::AgentToolResult;
use s_code_protocol::{
    Event, Id, PolicyDecision, PolicyResult, SubmitToolCall, ToolCall, ToolCallOutcome,
    ToolCallStatus, ToolRequest,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub(crate) fn definition() -> s_code_model_gateway::ToolDefinition {
    s_code_model_gateway::ToolDefinition {
        name: "execute".into(),
        description: "Run JavaScript to orchestrate read-only tools and filter their results. Available: tools.read_file, tools.list_files, tools.search_text, tools.git_status, tools.git_diff, using the same argument objects and result shapes as their direct tools. Use await or Promise.all and text(value) to return selected output. Example: const r = await tools.read_file({path: 'README.md'}); text(r); No imports, filesystem, network, shell, timers, or persistent state. Await every tool call. Limits: 30 seconds, 32 child calls per turn, 4 concurrent calls, 32 KiB source/output, 64 MiB JS heap. Errors reject the tool promise; catch them if useful. Tools needing approval must be called directly. Editing and commands always use direct tools. Each child call is visible and audited. Never automatically repeat a failed program; inspect the error first.".into(),
        parameters: json!({"type":"object","properties":{"code":{"type":"string","maxLength":MAX_CODE}},"required":["code"],"additionalProperties":false}),
    }
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl DaemonToolExecutor {
    pub(crate) async fn execute_code_mode(
        &self,
        call_id: &str,
        arguments: Value,
        cancellation: &CancellationToken,
    ) -> AgentToolResult {
        if !self.state.code_mode_enabled {
            return AgentToolResult::Failed {
                error: "Code Mode is disabled; use direct tools".into(),
            };
        }
        let code = match arguments
            .as_object()
            .filter(|args| args.len() == 1)
            .and_then(|args| args.get("code"))
            .and_then(Value::as_str)
        {
            Some(code) if !code.is_empty() && code.len() <= MAX_CODE => code.to_owned(),
            _ => {
                return AgentToolResult::Failed {
                    error: "execute requires only a nonempty code string of at most 32 KiB".into(),
                };
            }
        };
        let token = cancellation.child_token();
        // The supervising task owns cleanup even if AgentRunner drops this future.
        let _guard = CancelOnDrop(token.clone());
        let executor = self.clone();
        let call_id = call_id.to_owned();
        match tokio::spawn(async move {
            executor
                .code_mode_supervise(&call_id, code, token, &std::env::current_exe()?)
                .await
        })
        .await
        {
            Ok(Ok(value)) => AgentToolResult::Completed { value },
            Ok(Err(error)) => AgentToolResult::Failed {
                error: format!("{error:#}"),
            },
            Err(_) => AgentToolResult::Failed {
                error: "Code Mode supervisor stopped unexpectedly".into(),
            },
        }
    }

    async fn code_mode_event(
        &self,
        call: &ToolCall,
        kind: &str,
        model_call_id: Option<&str>,
    ) -> Result<()> {
        self.state.publish(Event {
            id: Id::new("evt"), sequence: 0, timestamp: chrono::Utc::now(),
            scope: self.scope.clone(), session_id: Some(self.session_id.clone()), turn_id: Some(self.turn_id.clone()),
            kind: kind.into(), payload: json!({
                "tool_call_id": call.request.id, "parent_tool_call_id": call.request.parent_tool_call_id,
                "model_call_id": model_call_id, "tool": call.request.tool,
                "display": crate::tool_activity_display(&call.request.tool, &call.request.arguments),
                "status": call.status,
            }),
        }).await.map_err(|error| anyhow::anyhow!("{error:?}"))?;
        Ok(())
    }

    async fn code_mode_supervise(
        &self,
        call_id: &str,
        code: String,
        token: CancellationToken,
        executable: &std::path::Path,
    ) -> Result<Value> {
        let _slot = tokio::select! {
            biased;
            _ = token.cancelled() => bail!("Code Mode cancelled"),
            permit = self.state.code_mode_slots.clone().acquire_owned() => permit?,
        };
        let parent = self
            .state
            .store
            .create_tool_call(
                ToolRequest {
                    id: Id::new("tool"),
                    parent_tool_call_id: None,
                    scope: self.scope.clone(),
                    session_id: self.session_id.clone(),
                    turn_id: self.turn_id.clone(),
                    tool: "execute".into(),
                    arguments: json!({"code":code}),
                    created_at: chrono::Utc::now(),
                },
                PolicyResult {
                    decision: PolicyDecision::Allow,
                    policy_id: "code-mode-read-only".into(),
                    policy_version: "1".into(),
                    reason:
                        "JavaScript orchestration only; every child tool is authorized separately"
                            .into(),
                    requires_approval: false,
                },
                Default::default(),
                ToolCallStatus::Running,
            )
            .await?;
        let result = async {
            self.code_mode_event(&parent, "tool.running", Some(call_id))
                .await?;
            self.code_mode_process(&parent.request.id, &code, &token, executable)
                .await
        };
        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(anyhow::anyhow!("Code Mode cancelled")),
            result = tokio::time::timeout(TIMEOUT, result) => result.unwrap_or_else(|_| Err(anyhow::anyhow!("Code Mode exceeded 30 seconds"))),
        };
        let status = if token.is_cancelled() {
            ToolCallStatus::Cancelled
        } else if result.is_ok() {
            ToolCallStatus::Completed
        } else {
            ToolCallStatus::Failed
        };
        let kind = match status {
            ToolCallStatus::Completed => "tool.completed",
            ToolCallStatus::Cancelled => "tool.cancelled",
            _ => "tool.failed",
        };
        let error = result.as_ref().err().map(|error| format!("{error:#}"));
        let (parent, cancelled_children) = self
            .state
            .store
            .finish_code_mode(
                &parent.request.id,
                status,
                result.as_ref().ok(),
                error.as_deref(),
            )
            .await?;
        for call in cancelled_children {
            self.code_mode_event(&call, "tool.cancelled", None).await?;
        }
        self.code_mode_event(&parent, kind, Some(call_id)).await?;
        result
    }

    async fn code_mode_process(
        &self,
        parent: &Id,
        code: &str,
        token: &CancellationToken,
        executable: &std::path::Path,
    ) -> Result<Value> {
        let used = self
            .state
            .store
            .code_mode_call_count(&self.scope, &self.session_id, &self.turn_id)
            .await?;
        let remaining = MAX_CALLS.saturating_sub(used);
        ensure!(
            remaining > 0,
            "Code Mode reached the 32-child-call turn limit; use direct tools"
        );
        let directory = tempfile::tempdir()?;
        let mut child = Command::new(executable)
            .arg("--code-mode-worker")
            .env_clear()
            .current_dir(directory.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().context("worker stdin unavailable")?;
        let mut output = BufReader::new(child.stdout.take().context("worker stdout unavailable")?);
        // Drain stdout independently: a large tool result must never block the
        // only reader while the worker is still writing a batch of requests.
        let (sender, mut receiver) = tokio::sync::mpsc::channel(MAX_CALLS + 1);
        let reader = tokio::spawn(async move {
            loop {
                let mut frame = Vec::new();
                let result = async {
                    let count = tokio::io::AsyncReadExt::take(&mut output, (MAX_FRAME + 1) as u64)
                        .read_until(b'\n', &mut frame)
                        .await?;
                    ensure!(count > 0, "Code Mode worker exited without a result");
                    ensure!(frame.len() <= MAX_FRAME, "Worker message exceeds limit");
                    Ok::<_, anyhow::Error>(serde_json::from_slice::<WorkerMessage>(&frame)?)
                }
                .await;
                let failed = result.is_err();
                if sender.send(result).await.is_err() || failed {
                    break;
                }
            }
        });
        struct AbortReader(tokio::task::AbortHandle);
        impl Drop for AbortReader {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _reader_guard = AbortReader(reader.abort_handle());
        input
            .write_all(format!("{}\n", json!({"code":code})).as_bytes())
            .await?;
        let mut pending = FuturesUnordered::new();
        let mut calls = 0;
        let result = async {
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => bail!("Code Mode cancelled"),
                    Some((id, response)) = pending.next(), if !pending.is_empty() => {
                        let value = match response { Ok(value) => json!({"id":id,"value":value}), Err(error) => json!({"id":id,"error":format!("{error:#}")}) };
                        let mut encoded = serde_json::to_vec(&value)?;
                        ensure!(encoded.len() < MAX_FRAME, "Tool result exceeds Code Mode's 1 MiB transfer limit; reduce max_bytes or use the direct tool");
                        encoded.push(b'\n');
                        input.write_all(&encoded).await?;
                    }
                    message = receiver.recv(), if pending.len() < 4 => {
                        let message = message.context("Worker reader disconnected")??;
                        match message {
                            WorkerMessage::Call { id, tool, arguments } => {
                                calls += 1;
                                ensure!(calls <= remaining && id as usize == calls, "Invalid worker call sequence or call limit exceeded");
                                ensure!(TOOLS.contains(&tool.as_str()) && arguments.is_object() && arguments.to_string().len() <= 64 * 1024, "Tool is not available in Code Mode");
                                pending.push(async move { (id, self.code_mode_child(parent, &tool, arguments, token).await) });
                            }
                            WorkerMessage::Done { output } => {
                                ensure!(pending.is_empty(), "Worker finished with unfinished tools");
                                ensure!(serde_json::to_vec(&output)?.len() <= MAX_OUTPUT + 256, "Code Mode output exceeds limit");
                                return Ok(json!({"output":output,"tool_calls":calls}));
                            }
                            WorkerMessage::Failed { error } => bail!("{error}"),
                        }
                    }
                }
            }
        }.await;
        // kill_on_drop covers cancellation of this future; explicit wait reaps the
        // child on ordinary completion and protocol errors.
        let _ = child.start_kill();
        let _ = child.wait().await;
        result
    }

    async fn code_mode_child(
        &self,
        parent: &Id,
        tool: &str,
        arguments: Value,
        token: &CancellationToken,
    ) -> Result<Value> {
        ensure!(!token.is_cancelled(), "Code Mode cancelled");
        let admitted = self
            .state
            .store
            .create_tool_call(
                ToolRequest {
                    id: Id::new("tool"),
                    parent_tool_call_id: Some(parent.clone()),
                    scope: self.scope.clone(),
                    session_id: self.session_id.clone(),
                    turn_id: self.turn_id.clone(),
                    tool: tool.into(),
                    arguments: arguments.clone(),
                    created_at: chrono::Utc::now(),
                },
                PolicyResult {
                    decision: PolicyDecision::Deny,
                    policy_id: "code-mode-admission".into(),
                    policy_version: "1".into(),
                    reason: "Awaiting tool validation and policy evaluation".into(),
                    requires_approval: false,
                },
                Default::default(),
                ToolCallStatus::Running,
            )
            .await?;
        self.code_mode_event(&admitted, "tool.running", None)
            .await?;
        let result = self
            .code_mode_child_admitted(parent, tool, arguments, token, &admitted.request.id)
            .await;
        if let Err(error) = &result {
            let current = self.state.store.get_tool_call(&admitted.request.id).await?;
            if matches!(
                current.status,
                ToolCallStatus::Proposed | ToolCallStatus::Running
            ) {
                let failed = self
                    .state
                    .store
                    .finish_tool_call(
                        &admitted.request.id,
                        ToolCallStatus::Failed,
                        None,
                        Some(&error.to_string()),
                    )
                    .await?;
                self.code_mode_event(&failed, "tool.failed", None).await?;
            }
        }
        result
    }

    async fn code_mode_child_admitted(
        &self,
        parent: &Id,
        tool: &str,
        arguments: Value,
        token: &CancellationToken,
        id: &Id,
    ) -> Result<Value> {
        ensure!(!token.is_cancelled(), "Code Mode cancelled");
        let initial = self
            .state
            .execution
            .preflight_for_turn(
                &self.session_id,
                &self.turn_id,
                SubmitToolCall {
                    scope: self.scope.clone(),
                    tool: tool.into(),
                    arguments: arguments.clone(),
                },
            )
            .await?;
        let (prepared, arguments) = if initial.decision() == &PolicyDecision::Allow {
            let arguments = self
                .run_hooks(
                    s_code_protocol::HookEvent::PreToolUse,
                    tool,
                    arguments,
                    None,
                    token,
                )
                .await
                .map_err(anyhow::Error::msg)?;
            let prepared = self
                .state
                .execution
                .preflight_for_turn(
                    &self.session_id,
                    &self.turn_id,
                    SubmitToolCall {
                        scope: self.scope.clone(),
                        tool: tool.into(),
                        arguments: arguments.clone(),
                    },
                )
                .await?;
            (prepared, arguments)
        } else {
            (initial, arguments)
        };
        ensure!(!token.is_cancelled(), "Code Mode cancelled");
        let prepared = prepared.with_parent(parent.clone());
        let outcome = self
            .state
            .execution
            .submit_prepared_without_approval(prepared, id.clone())
            .await?;
        crate::publish_tool_outcome_for_model_call(&self.state, &self.scope, &outcome, None)
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let result = match outcome {
            ToolCallOutcome::Completed { tool_call } => Ok(tool_call.result.unwrap_or(Value::Null)),
            ToolCallOutcome::Denied { tool_call } => {
                Err(anyhow::anyhow!("{}", tool_call.policy.reason))
            }
            ToolCallOutcome::Failed { tool_call } => Err(anyhow::anyhow!(
                "{}",
                tool_call.error.unwrap_or_else(|| "Tool failed".into())
            )),
            ToolCallOutcome::AwaitingApproval { .. } => bail!("Unexpected approval in Code Mode"),
        };
        let result_json = match &result {
            Ok(value) => json!({"outcome":"completed","value":value}),
            Err(error) => json!({"outcome":"failed","error":error.to_string()}),
        };
        if let Err(error) = self
            .run_hooks(
                s_code_protocol::HookEvent::PostToolUse,
                tool,
                arguments,
                Some(&result_json),
                token,
            )
            .await
        {
            tracing::warn!(%error, tool, "Code Mode Post Tool Use Hook failed");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, ToolProfile, build_transcript_snapshot_page};
    use s_code_protocol::{CreateSession, Scope, TranscriptItemContent, TurnStatus};

    async fn fixture() -> (tempfile::TempDir, DaemonToolExecutor) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=private\n").unwrap();
        let store = s_code_storage::Store::in_memory().await.unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("user".into()),
            goal_id: None,
            task_id: None,
        };
        let session = store
            .create_session(CreateSession {
                mode: s_code_protocol::SessionMode::Work,
                scope: scope.clone(),
                workspace_uri: url::Url::from_directory_path(dir.path())
                    .unwrap()
                    .to_string(),
                title: "Code Mode".into(),
                model: "test".into(),
            })
            .await
            .unwrap();
        let turn = store.create_turn(&scope, &session.id).await.unwrap();
        let state =
            AppState::new("test", store, 0).with_model_editing(&s_code_config::ModelConfig {
                code_mode: true,
                ..Default::default()
            });
        (
            dir,
            DaemonToolExecutor {
                state,
                session_id: session.id,
                turn_id: turn.id,
                scope,
            },
        )
    }
    fn executable() -> std::path::PathBuf {
        // `cargo test --workspace` builds the integration-test daemon binary.
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(if cfg!(windows) {
                "s-code-daemon.exe"
            } else {
                "s-code-daemon"
            })
    }
    async fn parent(executor: &DaemonToolExecutor) -> ToolCall {
        executor
            .state
            .store
            .create_tool_call(
                ToolRequest {
                    id: Id::new("tool"),
                    parent_tool_call_id: None,
                    scope: executor.scope.clone(),
                    session_id: executor.session_id.clone(),
                    turn_id: executor.turn_id.clone(),
                    tool: "execute".into(),
                    arguments: json!({"code":""}),
                    created_at: chrono::Utc::now(),
                },
                PolicyResult {
                    decision: PolicyDecision::Allow,
                    policy_id: "test".into(),
                    policy_version: "1".into(),
                    reason: "test".into(),
                    requires_approval: false,
                },
                Default::default(),
                ToolCallStatus::Running,
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn code_mode_is_optional_and_available_in_read_only_profiles() {
        let (_dir, mut executor) = fixture().await;
        for profile in [ToolProfile::Default, ToolProfile::Plan, ToolProfile::Review] {
            assert!(
                crate::tools_for_profile(&executor.state, profile)
                    .iter()
                    .any(|tool| tool.name == "execute")
            );
        }
        executor.state.code_mode_enabled = false;
        assert!(
            !crate::available_tools(&executor.state)
                .iter()
                .any(|tool| tool.name == "execute")
        );
        assert!(matches!(
            executor
                .execute_code_mode("call", json!({"code":"text(1)"}), &CancellationToken::new())
                .await,
            AgentToolResult::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn code_mode_children_keep_denials_validation_errors_and_paginated_parent_links() {
        let (_dir, executor) = fixture().await;
        let parent = parent(&executor).await;
        for (args, success) in [
            (json!({"path":"a.txt"}), true),
            (json!({"path":".env"}), false),
            (json!({"path":123}), false),
        ] {
            assert_eq!(
                executor
                    .code_mode_child(
                        &parent.request.id,
                        "read_file",
                        args,
                        &CancellationToken::new(),
                    )
                    .await
                    .is_ok(),
                success
            );
        }
        let calls = executor
            .state
            .store
            .list_tool_calls(&executor.scope, &executor.session_id)
            .await
            .unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[1].status, ToolCallStatus::Completed);
        assert_eq!(calls[2].status, ToolCallStatus::Failed);
        assert_eq!(calls[3].status, ToolCallStatus::Failed);
        for call in calls.iter().skip(1) {
            assert_eq!(
                call.request.parent_tool_call_id.as_ref(),
                Some(&parent.request.id)
            );
        }
        let page = build_transcript_snapshot_page(
            &executor.state,
            &executor.scope,
            &executor.session_id,
            None,
            1,
        )
        .await
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(
            matches!(&page.items[0].content,TranscriptItemContent::ToolCall {parent_tool_call_id:Some(id),..} if id==&parent.request.id)
        );
        let events = executor
            .state
            .store
            .list_session_events(&executor.scope, &executor.session_id, 100)
            .await
            .unwrap();
        assert!(events.iter().any(|e| e.kind == "tool.failed"
            && e.payload["parent_tool_call_id"] == json!(parent.request.id)));
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("SECRET=private")
        );
    }

    #[tokio::test]
    async fn code_mode_ask_never_creates_an_approval() {
        let (_dir, mut executor) = fixture().await;
        let parent = parent(&executor).await;
        let mut policy = s_code_policy::PolicyBundle::default();
        policy
            .rules
            .iter_mut()
            .find(|rule| rule.tool == "read_file")
            .unwrap()
            .decision = PolicyDecision::Ask;
        executor.state.execution = s_code_execution::ExecutionService::new(
            executor.state.store.clone(),
            policy,
            std::sync::Arc::new(s_code_platform_runtime::NativeRuntime),
        );
        let error = executor
            .code_mode_child(
                &parent.request.id,
                "read_file",
                json!({"path":"a.txt"}),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("call it directly"));
        assert!(
            executor
                .state
                .store
                .list_pending_session_approval_calls(&executor.scope, &executor.session_id)
                .await
                .unwrap()
                .is_empty()
        );
        let call = executor
            .state
            .store
            .list_tool_calls(&executor.scope, &executor.session_id)
            .await
            .unwrap();
        let call = call.last().unwrap();
        assert_eq!(call.status, ToolCallStatus::Denied);
        assert!(!call.policy.requires_approval);
    }

    #[tokio::test]
    async fn code_mode_large_parallel_transfers_do_not_deadlock() {
        let (dir, executor) = fixture().await;
        std::fs::write(dir.path().join("large.txt"), "x".repeat(128 * 1024)).unwrap();
        // Extra args exercise request-pipe backpressure; read_file ignores them.
        let code = "const calls = Array.from({length:8},()=>tools.read_file({path:'large.txt',max_bytes:200000,padding:'x'.repeat(32000)})); text((await Promise.all(calls)).length);";
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            executor.code_mode_supervise(
                "model-call",
                code.into(),
                CancellationToken::new(),
                &executable(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result["output"], json!([8]));
        let calls = executor
            .state
            .store
            .list_tool_calls(&executor.scope, &executor.session_id)
            .await
            .unwrap();
        assert_eq!(calls.len(), 9);
        assert!(
            calls
                .iter()
                .all(|call| call.status == ToolCallStatus::Completed)
        );
    }

    #[tokio::test]
    async fn code_mode_cancellation_and_restart_leave_no_running_lifecycles() {
        let (_dir, executor) = fixture().await;
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel.cancel();
        });
        let result = executor
            .code_mode_supervise("cancel-me", "while(true) {}".into(), token, &executable())
            .await;
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        let parent = parent(&executor).await;
        executor
            .state
            .store
            .update_turn(
                &executor.scope,
                &executor.turn_id,
                TurnStatus::RunningTool,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            executor
                .state
                .store
                .interrupt_code_mode_calls()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            executor
                .state
                .store
                .get_tool_call(&parent.request.id)
                .await
                .unwrap()
                .status,
            ToolCallStatus::Cancelled
        );
        assert_eq!(
            executor
                .state
                .store
                .get_turn(&executor.scope, &executor.turn_id)
                .await
                .unwrap()
                .status,
            TurnStatus::Cancelled
        );
    }
    #[tokio::test]
    async fn code_mode_atomic_stop_catches_unregistered_admissions_and_rejects_late_writes() {
        let (_dir, executor) = fixture().await;
        let parent = parent(&executor).await;
        // Simulate an INSERT committed before its caller received the result:
        // no in-memory child registry is involved in finalization.
        let mut request = parent.request.clone();
        request.id = Id::new("tool");
        request.parent_tool_call_id = Some(parent.request.id.clone());
        request.tool = "read_file".into();
        request.arguments = json!({"path":"a.txt"});
        let child = executor
            .state
            .store
            .create_tool_call(
                request.clone(),
                parent.policy.clone(),
                Default::default(),
                ToolCallStatus::Running,
            )
            .await
            .unwrap();
        let (_, cancelled) = executor
            .state
            .store
            .finish_code_mode(&parent.request.id, ToolCallStatus::Cancelled, None, None)
            .await
            .unwrap();
        assert_eq!(cancelled.len(), 1);
        let late = executor
            .state
            .store
            .finish_tool_call(
                &child.request.id,
                ToolCallStatus::Completed,
                Some(&json!({"late":true})),
                None,
            )
            .await
            .unwrap();
        assert_eq!(late.status, ToolCallStatus::Cancelled);
        assert!(late.result.is_none());
        request.id = Id::new("tool");
        assert!(
            executor
                .state
                .store
                .create_tool_call(
                    request,
                    parent.policy.clone(),
                    Default::default(),
                    ToolCallStatus::Running
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn code_mode_restart_preserves_later_direct_approval() {
        let (_dir, executor) = fixture().await;
        let parent = parent(&executor).await;
        executor
            .state
            .store
            .finish_code_mode(
                &parent.request.id,
                ToolCallStatus::Completed,
                Some(&json!({})),
                None,
            )
            .await
            .unwrap();
        let outcome = executor
            .state
            .execution
            .submit_for_turn(
                &executor.session_id,
                &executor.turn_id,
                SubmitToolCall {
                    scope: executor.scope.clone(),
                    tool: "run_command".into(),
                    arguments: json!({"program":"echo","args":["hello"]}),
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ToolCallOutcome::AwaitingApproval { .. }));
        executor
            .state
            .store
            .update_turn(
                &executor.scope,
                &executor.turn_id,
                TurnStatus::AwaitingApproval,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            executor
                .state
                .store
                .interrupt_code_mode_calls()
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            executor
                .state
                .store
                .get_turn(&executor.scope, &executor.turn_id)
                .await
                .unwrap()
                .status,
            TurnStatus::AwaitingApproval
        );
        assert_eq!(
            executor
                .state
                .store
                .list_pending_session_approval_calls(&executor.scope, &executor.session_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn code_mode_child_budget_accumulates_across_programs() {
        let (_dir, executor) = fixture().await;
        executor
            .code_mode_supervise(
                "first",
                "for(let i=0;i<31;i++) await tools.read_file({path:'a.txt'});".into(),
                CancellationToken::new(),
                &executable(),
            )
            .await
            .unwrap();
        let result = executor
            .code_mode_supervise(
                "second",
                "await tools.read_file({path:'a.txt'}); await tools.read_file({path:'a.txt'});"
                    .into(),
                CancellationToken::new(),
                &executable(),
            )
            .await;
        assert!(result.unwrap_err().to_string().contains("call limit"));
        assert_eq!(
            executor
                .state
                .store
                .code_mode_call_count(&executor.scope, &executor.session_id, &executor.turn_id)
                .await
                .unwrap(),
            32
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn code_mode_hooks_keep_the_direct_tool_result_contract() {
        let (dir, executor) = fixture().await;
        let parent = parent(&executor).await;
        let capture = dir.path().join("hook-input.json");
        let hook = s_code_protocol::HookSpec {
            id: "capture".into(),
            name: "Capture result".into(),
            event: s_code_protocol::HookEvent::PostToolUse,
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "cat > \"$1\"; printf '{}'".into(),
                "hook".into(),
                capture.to_string_lossy().into_owned(),
            ],
            environment_handles: Default::default(),
            timeout_ms: 1000,
            can_modify_input: false,
        };
        let digest = crate::hook_install_preview(&hook)
            .unwrap()
            .permissions_sha256;
        executor
            .state
            .store
            .install_hook(&executor.scope, &hook, &digest)
            .await
            .unwrap();
        executor
            .code_mode_child(
                &parent.request.id,
                "read_file",
                json!({"path":"a.txt"}),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let input: Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
        assert_eq!(input["result"]["outcome"], "completed");
    }
}
