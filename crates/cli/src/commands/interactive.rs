use super::slash::{
    complete_slash_command, move_slash_command_selection, slash_command_input_is_exact,
    slash_command_menu_visible,
};
use crate::state::{InputMode, QuestionActivity};
use crate::*;
use std::collections::BTreeMap;

pub(crate) async fn run_command(api: &Api, app: &mut App, command: &str) {
    let command = command.trim();
    match command.split_whitespace().next().unwrap_or(command) {
        "/help" | "/" => {
            app.activity.push_front(
                "Commands · /new /resume /fork /retry /checkpoints /rename /alias /goal /goal-run /terminal /ps /stop /side /btw /agents /agent /subagents /follow-up /wait /interrupt /close-agent /archive /unarchive /delete /steer /queue /dequeue /attach /detach /diff /undo /copy /raw /output /links /usage /editor /keymap /vim /theme /statusline /context /compact /memory /learn /init /answer /artifact /model /permissions /status /clear /exit /help".into(),
            );
            app.status = "type a command and press Enter".into();
        }
        "/steer" | "/queue" => {
            let mode = if command.starts_with("/steer") {
                TurnInputMode::Steer
            } else {
                TurnInputMode::Queue
            };
            let prompt = command
                .split_once(char::is_whitespace)
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            if prompt.is_empty() {
                app.status = format!(
                    "usage: /{} <message>",
                    if mode == TurnInputMode::Steer {
                        "steer"
                    } else {
                        "queue"
                    }
                );
            } else if !app.turn_running {
                app.status = "steer and queue require a running turn".into();
            } else {
                submit_pending_input(api, app, prompt.into(), mode).await;
            }
        }
        "/dequeue" => {
            let input_id = command
                .strip_prefix("/dequeue")
                .map(str::trim)
                .unwrap_or_default();
            if input_id.is_empty() {
                app.status = "usage: /dequeue <input-id>".into();
            } else {
                match api.cancel_turn_input(&Id(input_id.into())).await {
                    Ok(cancelled) => {
                        app.pending_inputs
                            .retain(|candidate| candidate.id != cancelled.id);
                        app.status = "queued input removed".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/attach" => {
            let path = command
                .strip_prefix("/attach")
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let Some(path) = path else {
                app.status = "usage: /attach <file-path>".into();
                return;
            };
            if app.turn_running {
                app.status = "wait for the running turn before attaching a file".into();
                return;
            }
            if app.pending_attachments.len() >= 8 {
                app.status = "a message can contain at most eight attachments".into();
                return;
            }
            let Some(session_id) = app.current().map(|session| session.id.clone()) else {
                app.status = "create a session before attaching a file".into();
                return;
            };
            let path = match fs::canonicalize(path) {
                Ok(path) => path,
                Err(error) => {
                    app.activity.push_front(format!("× {error}"));
                    return;
                }
            };
            let metadata = match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => metadata,
                Ok(_) => {
                    app.status = "attachments must be regular files".into();
                    return;
                }
                Err(error) => {
                    app.activity.push_front(format!("× {error}"));
                    return;
                }
            };
            if metadata.len() == 0 || metadata.len() > 5 * 1024 * 1024 {
                app.status = "attachment size must be between 1 byte and 5 MiB".into();
                return;
            }
            let Some(file_name) = path
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
            else {
                app.status = "attachment file name must be valid UTF-8".into();
                return;
            };
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    app.activity.push_front(format!("× {error}"));
                    return;
                }
            };
            match api
                .upload_attachment(
                    &session_id,
                    file_name.clone(),
                    attachment_media_type(&path).into(),
                    STANDARD.encode(bytes),
                )
                .await
            {
                Ok(attachment) => {
                    app.pending_attachments.push(attachment);
                    app.status = format!(
                        "attached {file_name} · {} of 8",
                        app.pending_attachments.len()
                    );
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/detach" => {
            let value = command
                .strip_prefix("/detach")
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let Some(value) = value else {
                app.status = "usage: /detach <attachment-id|number>".into();
                return;
            };
            let attachment = value
                .parse::<usize>()
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| app.pending_attachments.get(index).cloned())
                .or_else(|| {
                    app.pending_attachments
                        .iter()
                        .find(|attachment| attachment.id.0 == value)
                        .cloned()
                });
            let Some(attachment) = attachment else {
                app.status = format!("no pending attachment matches {value:?}");
                return;
            };
            match api.delete_attachment(&attachment.id).await {
                Ok(()) => {
                    app.pending_attachments
                        .retain(|candidate| candidate.id != attachment.id);
                    app.status = format!("detached {}", attachment.file_name);
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/new" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before creating another session".into();
                return;
            }
            app.input_mode = InputMode::NewWorkspace;
            app.pending_workspace = None;
            app.status = "enter workspace file:// URI".into();
        }
        "/clear" => {
            app.messages.clear();
            app.activity.clear();
            app.tool_activity.clear();
            app.tool_result.clear();
            app.status = "local transcript cleared; session history is preserved".into();
        }
        "/model" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before changing the model".into();
                return;
            }
            match api.model_catalog().await {
                Ok(models) if models.is_empty() => {
                    app.status = "the server has no models available for this Team".into();
                }
                Ok(models) => {
                    app.picker = Some(PickerState {
                        kind: PickerKind::Model,
                        title: "Choose a server-approved model".into(),
                        options: models
                            .into_iter()
                            .map(|model| PickerOption {
                                id: model.id.clone(),
                                label: model.display_name,
                                detail: format!(
                                    "{} · {}{}",
                                    model.provider,
                                    model.source.replace('_', " "),
                                    if model.recommended {
                                        " · recommended"
                                    } else {
                                        ""
                                    }
                                ),
                                disabled_reason: if model.available {
                                    None
                                } else {
                                    model
                                        .locked_reason
                                        .or_else(|| Some("model is unavailable".into()))
                                },
                            })
                            .collect(),
                        query: command
                            .strip_prefix("/model")
                            .map(str::trim)
                            .unwrap_or_default()
                            .to_owned(),
                        selected: 0,
                    });
                    app.status = "model picker · type to search".into();
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/resume" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before switching sessions".into();
                return;
            }
            let query = command
                .strip_prefix("/resume")
                .map(str::trim)
                .filter(|query| !query.is_empty());
            let next = query
                .and_then(|query| {
                    app.sessions.iter().position(|session| {
                        session.id.0 == query
                            || session.title.to_lowercase().contains(&query.to_lowercase())
                    })
                })
                .or_else(|| {
                    (app.sessions.len() > 1).then(|| (app.selected + 1) % app.sessions.len())
                });
            if let Some(next) = next {
                app.selected = next;
                app.pending_inputs.clear();
                if let Some(session) = app.current() {
                    let id = session.id.clone();
                    let title = session.title.clone();
                    match load_session_state(api, app, &id).await {
                        Ok(()) => {
                            load_session_preferences(api, app, &id).await;
                            load_session_goal(api, app, &id).await;
                            app.status = format!("resumed {title}");
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
            } else if let Some(query) = query {
                app.status = format!("no session matches {query:?}");
            } else {
                app.status = "no other session to resume".into();
            }
        }
        "/fork" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before forking".into();
                return;
            }
            let title = command
                .strip_prefix("/fork")
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            if let Some(source_id) = app.current().map(|session| session.id.clone()) {
                match api.fork_session(&source_id, title).await {
                    Ok(fork) => {
                        app.sessions.insert(0, fork.clone());
                        app.selected = 0;
                        app.pending_inputs.clear();
                        match load_session_state(api, app, &fork.id).await {
                            Ok(()) => {
                                load_session_preferences(api, app, &fork.id).await;
                                load_session_goal(api, app, &fork.id).await;
                                app.status = format!("forked conversation into {}", fork.title);
                            }
                            Err(error) => app.activity.push_front(format!("× {error}")),
                        }
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else {
                app.status = "select a session before forking".into();
            }
        }
        "/retry" => {
            if app.turn_running {
                app.status = "stop the running turn before retrying".into();
                return;
            }
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before retrying".into();
                return;
            }
            let arguments = command
                .strip_prefix("/retry")
                .map(str::trim)
                .unwrap_or_default();
            let mut parts = arguments.splitn(2, char::is_whitespace);
            let requested_turn = parts
                .next()
                .filter(|value| !value.is_empty())
                .map(|value| Id(value.to_owned()));
            let edited = parts
                .next()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| serde_json::Value::String(value.to_owned()));
            let Some(turn_id) = requested_turn.or_else(|| app.current_turn.clone()) else {
                app.status = "usage: /retry <turn-id> [edited message]".into();
                return;
            };
            match api.retry_turn(&turn_id, edited).await {
                Ok(result) => {
                    app.sessions.insert(0, result.session.clone());
                    app.selected = 0;
                    app.pending_inputs.clear();
                    match load_session_state(api, app, &result.session.id).await {
                        Ok(()) => {
                            load_session_preferences(api, app, &result.session.id).await;
                            load_session_goal(api, app, &result.session.id).await;
                            app.current_turn = Some(result.turn.id);
                            app.turn_running = true;
                            app.status =
                                format!("retry started in new branch {}", result.session.title);
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/checkpoints" => {
            if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api.turns(&session_id).await {
                    Ok(turns) if turns.is_empty() => {
                        app.tool_result = "No checkpoints in this session.".into();
                        app.status = "no checkpoints".into();
                    }
                    Ok(turns) => {
                        app.tool_result = turns
                            .iter()
                            .rev()
                            .map(|turn| {
                                let marker = if app.current_turn.as_ref() == Some(&turn.id) {
                                    "current"
                                } else {
                                    "checkpoint"
                                };
                                format!("{}  {:?}  {marker}", turn.id.0, turn.status)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        app.status = format!(
                            "{} checkpoint{} · /undo <turn-id> restores one",
                            turns.len(),
                            if turns.len() == 1 { "" } else { "s" }
                        );
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else {
                app.status = "select a session to browse checkpoints".into();
            }
        }
        "/rename" => {
            let title = command
                .strip_prefix("/rename")
                .map(str::trim)
                .unwrap_or_default();
            if title.is_empty() {
                app.status = "usage: /rename <new title>".into();
            } else if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api
                    .update_session(&session_id, Some(title.into()), None)
                    .await
                {
                    Ok(updated) => {
                        app.sessions[app.selected] = updated;
                        app.status = "session renamed".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/alias" => {
            let alias = command
                .strip_prefix("/alias")
                .map(str::trim)
                .unwrap_or_default();
            if alias.is_empty() {
                app.status = "usage: /alias <assistant name>".into();
            } else if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api
                    .update_session_preferences(
                        &session_id,
                        UpdateSessionPreferences {
                            scope: api.scope.clone(),
                            permission_mode: None,
                            assistant_alias: Some(alias.into()),
                        },
                    )
                    .await
                {
                    Ok(preferences) => {
                        app.assistant_alias = preferences.assistant_alias;
                        app.status = format!("assistant alias: {}", app.assistant_alias);
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/goal" => {
            let arguments = command
                .strip_prefix("/goal")
                .map(str::trim)
                .unwrap_or_default();
            let Some(session_id) = app.current().map(|session| session.id.clone()) else {
                app.status = "select a session before managing a Goal".into();
                return;
            };
            let (action, value) = arguments
                .split_once(char::is_whitespace)
                .map(|(action, value)| (action, value.trim()))
                .unwrap_or((arguments, ""));
            match action {
                "" | "view" | "status" => {
                    if let Some(goal) = app.goal.as_ref() {
                        app.activity.push_front(format!(
                            "Goal · {:?} · {} · {} turns · {} tokens{}",
                            goal.status,
                            goal.objective,
                            goal.continuation_count,
                            goal.input_tokens.saturating_add(goal.output_tokens),
                            goal.blocked_reason
                                .as_ref()
                                .map(|reason| format!(" · {reason}"))
                                .unwrap_or_default()
                        ));
                        app.status =
                            "Goal controls: /goal edit, /goal pause, /goal resume, /goal clear"
                                .into();
                    } else {
                        app.status = "no Goal · use /goal <objective> to start one".into();
                    }
                }
                "edit" => {
                    let Some(goal) = app.goal.as_ref() else {
                        app.status = "no Goal to edit".into();
                        return;
                    };
                    if value.is_empty() {
                        app.status = "usage: /goal edit <objective>".into();
                        return;
                    }
                    match api
                        .update_session_goal(
                            &session_id,
                            UpdateSessionGoal {
                                scope: api.scope.clone(),
                                objective: Some(value.into()),
                                status: None,
                                auto_continue: None,
                                expected_revision: goal.revision,
                                blocked_reason: None,
                            },
                        )
                        .await
                    {
                        Ok(goal) => {
                            app.goal = Some(goal);
                            app.status = "Goal objective updated".into();
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "pause" | "resume" => {
                    let Some(goal) = app.goal.as_ref() else {
                        app.status = "no Goal to update".into();
                        return;
                    };
                    let status = if action == "pause" {
                        SessionGoalStatus::Paused
                    } else {
                        SessionGoalStatus::Active
                    };
                    match api
                        .update_session_goal(
                            &session_id,
                            UpdateSessionGoal {
                                scope: api.scope.clone(),
                                objective: None,
                                status: Some(status),
                                auto_continue: None,
                                expected_revision: goal.revision,
                                blocked_reason: None,
                            },
                        )
                        .await
                    {
                        Ok(goal) => {
                            app.goal = Some(goal);
                            app.status = format!("Goal {action}d");
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "clear" => {
                    let Some(goal) = app.goal.as_ref() else {
                        app.status = "no Goal to clear".into();
                        return;
                    };
                    match api.clear_session_goal(&session_id, goal.revision).await {
                        Ok(_) => {
                            app.goal = None;
                            app.status = "Goal cleared".into();
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                _ => {
                    if arguments.is_empty() {
                        app.status = "usage: /goal <objective>".into();
                        return;
                    }
                    match api.set_session_goal(&session_id, arguments.into()).await {
                        Ok(goal) => {
                            app.goal = Some(goal);
                            app.status = "Goal started".into();
                            if !app.turn_running {
                                start_prompt(api, app, arguments.into()).await;
                            }
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
            }
        }
        "/goal-run" => {
            let arguments = command
                .strip_prefix("/goal-run")
                .map(str::trim)
                .unwrap_or_default();
            let mut parts = arguments.split_whitespace();
            let first = parts.next().unwrap_or_default();
            let (action, goal_id) = if matches!(first, "list" | "pause" | "resume" | "cancel") {
                (first, parts.next().unwrap_or_default())
            } else {
                ("list", first)
            };
            let run_id = parts.next();
            if goal_id.is_empty() || parts.next().is_some() {
                app.status =
                    "usage: /goal-run [list|pause|resume|cancel] <goal-id> [run-id]".into();
                return;
            }
            match api.team_goal_runs(&Id(goal_id.into())).await {
                Ok(mut runs) => {
                    runs.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                    if action == "list" {
                        app.tool_result = if runs.is_empty() {
                            "No Goal runs.".into()
                        } else {
                            runs.iter()
                                .map(|run| {
                                    format!(
                                        "{}  {:?}  task {}  rev {}  ${:.2}/Task",
                                        run.id.0,
                                        run.status,
                                        run.current_task_id
                                            .as_ref()
                                            .map(|id| id.0.as_str())
                                            .unwrap_or("none"),
                                        run.revision,
                                        run.max_cost_micros as f64 / 1_000_000.0,
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
                        app.status =
                            "Goal runs · use /goal-run pause|resume|cancel <goal-id> [run-id]"
                                .into();
                        return;
                    }
                    let selected = run_id
                        .and_then(|id| runs.iter().find(|run| run.id.0 == id))
                        .or_else(|| {
                            runs.iter()
                                .find(|run| run.status != TeamGoalRunStatus::Cancelled)
                        });
                    let Some(run) = selected else {
                        app.status = "Goal run was not found".into();
                        return;
                    };
                    let status = match action {
                        "pause" => TeamGoalRunStatus::Paused,
                        "resume" => TeamGoalRunStatus::Active,
                        "cancel" => TeamGoalRunStatus::Cancelled,
                        _ => unreachable!(),
                    };
                    match api
                        .control_team_goal_run(&Id(goal_id.into()), &run.id, status, run.revision)
                        .await
                    {
                        Ok(updated) => {
                            app.tool_result = format!(
                                "Goal run: {}\nStatus: {:?}\nCurrent task: {}\nRevision: {}",
                                updated.id.0,
                                updated.status,
                                updated
                                    .current_task_id
                                    .as_ref()
                                    .map(|id| id.0.as_str())
                                    .unwrap_or("none"),
                                updated.revision,
                            );
                            app.status = format!("Goal run {action}d");
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/terminal" => {
            let arguments = command
                .strip_prefix("/terminal")
                .map(str::trim)
                .unwrap_or_default();
            let (action, value) = arguments
                .split_once(char::is_whitespace)
                .map(|(action, value)| (action, value.trim()))
                .unwrap_or((arguments, ""));
            match action {
                "" | "list" => match api.background_terminals().await {
                    Ok(mut terminals) => {
                        terminals.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                        app.tool_result = if terminals.is_empty() {
                            "No background terminals.".into()
                        } else {
                            terminals
                                .iter()
                                .take(20)
                                .map(|terminal| {
                                    format!(
                                        "{}  {:?}  {}  {}x{}  {} bytes{}",
                                        terminal.id.0,
                                        terminal.status,
                                        terminal.program,
                                        terminal.cols,
                                        terminal.rows,
                                        terminal.output_byte_length,
                                        terminal
                                            .exit_code
                                            .map(|code| format!("  exit {code}"))
                                            .unwrap_or_default(),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
                        app.status = format!(
                            "{} terminal{} · /terminal output|write|resize|stop",
                            terminals.len(),
                            if terminals.len() == 1 { "" } else { "s" }
                        );
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                },
                "start" => {
                    let Some(session) = app.current().cloned() else {
                        app.status = "select a Session before starting a terminal".into();
                        return;
                    };
                    let words = match shell_words::split(value) {
                        Ok(words) if !words.is_empty() => words,
                        Ok(_) => {
                            app.status =
                                "usage: /terminal start <absolute-program> [arg ...]".into();
                            return;
                        }
                        Err(error) => {
                            app.status = format!("terminal arguments are invalid: {error}");
                            return;
                        }
                    };
                    let terminal = BackgroundTerminalSpec {
                        session_id: session.id,
                        program: words[0].clone(),
                        args: words[1..].to_vec(),
                        environment_handles: BTreeMap::new(),
                        working_directory_uri: session.workspace_uri,
                        rows: 24,
                        cols: 80,
                        max_runtime_seconds: 3_600,
                    };
                    match api.preview_background_terminal(terminal.clone()).await {
                        Ok(preview) => {
                            let permissions = preview
                                .permissions
                                .iter()
                                .map(|permission| {
                                    format!(
                                        "{}  {}\n    {}",
                                        mcp_permission_kind_name(&permission.kind),
                                        permission.value,
                                        permission.reason
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            app.tool_result = format!(
                                "Background terminal permission preview\n{permissions}\n\nConfirmation digest:\n{}",
                                preview.permissions_sha256
                            );
                            app.status = format!(
                                "review permissions, then /terminal confirm {}",
                                preview.permissions_sha256
                            );
                            app.pending_terminal = Some((terminal, preview));
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "confirm" => {
                    let Some((terminal, preview)) = app.pending_terminal.clone() else {
                        app.status = "there is no pending terminal permission preview".into();
                        return;
                    };
                    if value != preview.permissions_sha256 {
                        app.status =
                            "confirmation digest does not match the displayed permission preview"
                                .into();
                        return;
                    }
                    match api
                        .start_background_terminal(terminal, preview.permissions_sha256)
                        .await
                    {
                        Ok(terminal) => {
                            app.pending_terminal = None;
                            app.status = format!(
                                "background terminal {} is {:?}",
                                terminal.id.0, terminal.status
                            );
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "output" => {
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        app.status = "usage: /terminal output <terminal-id>".into();
                        return;
                    }
                    let terminal_id = Id(value.into());
                    let mut offset = 0;
                    let mut output = Vec::new();
                    loop {
                        match api.background_terminal_output(&terminal_id, offset).await {
                            Ok(chunk) => {
                                match STANDARD.decode(chunk.content_base64) {
                                    Ok(bytes) => output.extend(bytes),
                                    Err(error) => {
                                        app.activity.push_front(format!(
                                            "× terminal output was not valid base64: {error}"
                                        ));
                                        return;
                                    }
                                }
                                if chunk.eof || chunk.next_offset == offset {
                                    break;
                                }
                                offset = chunk.next_offset;
                            }
                            Err(error) => {
                                app.activity.push_front(format!("× {error}"));
                                return;
                            }
                        }
                    }
                    app.tool_result = if output.is_empty() {
                        "Terminal has not produced output.".into()
                    } else {
                        String::from_utf8_lossy(&output).into_owned()
                    };
                    app.status = format!("loaded {} terminal output bytes", output.len());
                }
                "write" => {
                    let Some((terminal_id, content)) = value.split_once(char::is_whitespace) else {
                        app.status = "usage: /terminal write <terminal-id> <text>".into();
                        return;
                    };
                    let content = format!("{}\n", content.trim_start());
                    match api
                        .write_background_terminal(&Id(terminal_id.into()), content.as_bytes())
                        .await
                    {
                        Ok(terminal) => {
                            app.status = format!("input sent to terminal {}", terminal.id.0);
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "resize" => {
                    let parts = value.split_whitespace().collect::<Vec<_>>();
                    if parts.len() != 3 {
                        app.status = "usage: /terminal resize <terminal-id> <rows> <cols>".into();
                        return;
                    }
                    let dimensions = parts[1]
                        .parse::<u16>()
                        .ok()
                        .zip(parts[2].parse::<u16>().ok());
                    let Some((rows, cols)) = dimensions else {
                        app.status = "terminal rows and columns must be integers".into();
                        return;
                    };
                    match api
                        .resize_background_terminal(&Id(parts[0].into()), rows, cols)
                        .await
                    {
                        Ok(terminal) => {
                            app.status =
                                format!("terminal {} resized to {cols}x{rows}", terminal.id.0);
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                "stop" => {
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        app.status = "usage: /terminal stop <terminal-id>".into();
                        return;
                    }
                    match api.background_terminals().await {
                        Ok(terminals) => {
                            let Some(terminal) = terminals
                                .into_iter()
                                .find(|terminal| terminal.id.0 == value)
                            else {
                                app.status = "background terminal was not found".into();
                                return;
                            };
                            match api
                                .stop_background_terminal(&terminal.id, terminal.revision)
                                .await
                            {
                                Ok(terminal) => {
                                    app.status =
                                        format!("stop requested for terminal {}", terminal.id.0);
                                }
                                Err(error) => app.activity.push_front(format!("× {error}")),
                            }
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                _ => {
                    app.status = "usage: /terminal [list|start <program> [arg ...]|confirm <digest>|output <id>|write <id> <text>|resize <id> <rows> <cols>|stop <id>]".into();
                }
            }
        }
        "/ps" | "/stop" => {
            let arguments = command
                .split_once(char::is_whitespace)
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            let mut parts = arguments.split_whitespace();
            let mut action = parts.next().unwrap_or_default();
            let mut task_id = parts.next().unwrap_or_default();
            if command.starts_with("/stop") {
                task_id = action;
                action = "cancel";
            }
            if command.starts_with("/stop") && task_id.is_empty() {
                app.status = "usage: /stop <task-or-terminal-id>".into();
                return;
            }
            if action.is_empty() || action == "list" {
                let (tasks, terminals) =
                    tokio::join!(api.durable_tasks(), api.background_terminals());
                match (tasks, terminals) {
                    (Ok(mut tasks), Ok(mut terminals)) => {
                        tasks.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                        terminals.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                        app.tool_result = if tasks.is_empty() && terminals.is_empty() {
                            "No background work.".into()
                        } else {
                            let mut lines = tasks
                                .iter()
                                .take(20)
                                .map(|task| {
                                    let session = task
                                        .session_id
                                        .as_ref()
                                        .and_then(|id| {
                                            app.sessions.iter().find(|session| &session.id == id)
                                        })
                                        .map(|session| session.title.as_str())
                                        .unwrap_or(task.kind.as_str());
                                    format!(
                                        "{}  {:?}  {session}  attempt {}/{}  ${:.4}",
                                        task.id.0,
                                        task.status,
                                        task.attempt,
                                        task.max_attempts,
                                        task.consumed_cost_micros as f64 / 1_000_000.0
                                    )
                                })
                                .collect::<Vec<_>>();
                            lines.extend(terminals.iter().take(20).map(|terminal| {
                                format!(
                                    "{}  PTY/{:?}  {}  {} bytes",
                                    terminal.id.0,
                                    terminal.status,
                                    terminal.program,
                                    terminal.output_byte_length
                                )
                            }));
                            lines.join("\n")
                        };
                        app.status = format!(
                            "{} task{} · {} terminal{} · /stop <id>",
                            tasks.len(),
                            if tasks.len() == 1 { "" } else { "s" },
                            terminals.len(),
                            if terminals.len() == 1 { "" } else { "s" },
                        );
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        app.activity.push_front(format!("× {error}"))
                    }
                }
            } else if !matches!(action, "pause" | "resume" | "cancel") || task_id.is_empty() {
                app.status = if command.starts_with("/stop") {
                    "usage: /stop <task-or-terminal-id>".into()
                } else {
                    "usage: /ps [list|pause <id>|resume <id>|cancel <id>]".into()
                };
            } else {
                if command.starts_with("/stop") {
                    match api.background_terminals().await {
                        Ok(terminals) => {
                            if let Some(terminal) = terminals
                                .into_iter()
                                .find(|terminal| terminal.id.0 == task_id)
                            {
                                match api
                                    .stop_background_terminal(&terminal.id, terminal.revision)
                                    .await
                                {
                                    Ok(terminal) => {
                                        app.status = format!(
                                            "stop requested for terminal {}",
                                            terminal.id.0
                                        );
                                    }
                                    Err(error) => app.activity.push_front(format!("× {error}")),
                                }
                                return;
                            }
                        }
                        Err(error) => {
                            app.activity.push_front(format!("× {error}"));
                            return;
                        }
                    }
                }
                match api.control_durable_task(&Id(task_id.into()), action).await {
                    Ok(task) => {
                        app.status = format!("background task {} is {:?}", task.id.0, task.status);
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/side" | "/btw" => {
            let arguments = command
                .split_once(char::is_whitespace)
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            let conversations = match api.side_conversations().await {
                Ok(conversations) => conversations,
                Err(error) => {
                    app.activity.push_front(format!("× {error}"));
                    return;
                }
            };
            let current_session_id = app.current().map(|session| session.id.clone());
            let current_side = current_session_id.as_ref().and_then(|session_id| {
                conversations
                    .iter()
                    .find(|conversation| {
                        &conversation.session_id == session_id
                            && conversation.status
                                == s_code_protocol::SideConversationStatus::Active
                    })
                    .cloned()
            });
            if arguments.is_empty() || arguments == "list" {
                let active = conversations
                    .iter()
                    .filter(|conversation| {
                        conversation.status == s_code_protocol::SideConversationStatus::Active
                    })
                    .collect::<Vec<_>>();
                app.tool_result = if active.is_empty() {
                    "No active side conversations.".into()
                } else {
                    active
                        .iter()
                        .map(|conversation| {
                            let title = app
                                .sessions
                                .iter()
                                .find(|session| session.id == conversation.session_id)
                                .map(|session| session.title.as_str())
                                .unwrap_or("Side conversation");
                            format!(
                                "{}  {title}  source {}",
                                conversation.id.0, conversation.source_session_id.0
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                app.status = "use /side <question>, /side promote, or /side close".into();
            } else if matches!(arguments, "promote" | "close") {
                let Some(side) = current_side else {
                    app.status = "the current Session is not an active side conversation".into();
                    return;
                };
                let result = if arguments == "promote" {
                    api.promote_side_conversation(&side.id).await
                } else {
                    api.close_side_conversation(&side.id).await
                };
                match result {
                    Ok(_) if arguments == "promote" => {
                        app.status = "side conversation promoted to a regular Session".into();
                    }
                    Ok(_) => {
                        let source_id = side.source_session_id;
                        if let Some(index) = app
                            .sessions
                            .iter()
                            .position(|session| session.id == source_id)
                        {
                            app.selected = index;
                            app.clear_transcript();
                            if let Err(error) = load_session_state(api, app, &source_id).await {
                                app.activity.push_front(format!("× {error}"));
                            }
                            load_session_preferences(api, app, &source_id).await;
                            load_session_goal(api, app, &source_id).await;
                        }
                        app.status = "side conversation closed; source Session unchanged".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else if app.turn_running {
                app.status = "wait for the current Turn before starting a side conversation".into();
            } else if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before starting a side conversation".into();
            } else {
                let Some(source_session) = current_session_id else {
                    app.status = "select a Session before starting a side conversation".into();
                    return;
                };
                match api
                    .create_side_conversation(&source_session, arguments.into())
                    .await
                {
                    Ok(result) => {
                        if let Some(index) = app
                            .sessions
                            .iter()
                            .position(|session| session.id == result.session.id)
                        {
                            app.sessions[index] = result.session.clone();
                            app.selected = index;
                        } else {
                            app.sessions.push(result.session.clone());
                            app.selected = app.sessions.len() - 1;
                        }
                        app.clear_transcript();
                        let _ = load_session_state(api, app, &result.session.id).await;
                        load_session_preferences(api, app, &result.session.id).await;
                        load_session_goal(api, app, &result.session.id).await;
                        app.current_turn = Some(result.turn.id);
                        app.turn_running = true;
                        app.status =
                            "side conversation started · /side promote or /side close".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/agents" => {
            let action = command
                .strip_prefix("/agents")
                .map(str::trim)
                .unwrap_or_default();
            if action == "results" {
                match api.agent_results().await {
                    Ok(results) => {
                        app.tool_result = render_agent_results(&results);
                        app.status =
                            "Bounded final Agent results · /agent switch <id> opens full history"
                                .into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
                return;
            }
            match api.agent_runs().await {
                Ok(mut agents) => {
                    agents.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                    app.tool_result = if action.is_empty() {
                        agent_tree(&agents, None)
                    } else {
                        app.status = "usage: /agents [results]".into();
                        return;
                    };
                    app.status = "Agent tree · /agent <id> · /agent result <id> · /agent switch <id> · /agent message <id> <message> · /agents results".into();
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/subagents" => {
            let parent = command
                .strip_prefix("/subagents")
                .map(str::trim)
                .unwrap_or_default();
            match api.agent_runs().await {
                Ok(mut agents) => {
                    agents.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
                    let parent_id = (!parent.is_empty()).then(|| Id(parent.into()));
                    app.tool_result = agent_tree(&agents, parent_id.as_ref());
                    app.status = if parent.is_empty() {
                        "Subagents · provide a parent ID to filter".into()
                    } else {
                        format!("Direct children of Agent {parent}")
                    };
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/agent" => {
            let arguments = command
                .strip_prefix("/agent")
                .map(str::trim)
                .unwrap_or_default();
            if let Some(id) = arguments.strip_prefix("switch ").map(str::trim) {
                if id.is_empty() || id.contains(char::is_whitespace) {
                    app.status = "usage: /agent switch <agent-id>".into();
                    return;
                }
                switch_to_agent_session(api, app, &Id(id.into())).await;
                return;
            }
            if let Some(id) = arguments.strip_prefix("result ").map(str::trim) {
                if id.is_empty() || id.contains(char::is_whitespace) {
                    app.status = "usage: /agent result <agent-id>".into();
                    return;
                }
                match api.agent_result(&Id(id.into())).await {
                    Ok(result) => {
                        app.tool_result = render_agent_results(&[result]);
                        app.status =
                            "Bounded final Agent result · /agent switch <id> opens full history"
                                .into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
                return;
            }
            if let Some(arguments) = arguments.strip_prefix("message ").map(str::trim) {
                let Some((id, content)) = arguments.split_once(char::is_whitespace) else {
                    app.status = "usage: /agent message <agent-id> <message>".into();
                    return;
                };
                let content = content.trim();
                if content.is_empty() {
                    app.status = "usage: /agent message <agent-id> <message>".into();
                    return;
                }
                send_agent_input(api, app, &Id(id.into()), content.into()).await;
                return;
            }
            if arguments.is_empty() || arguments.contains(char::is_whitespace) {
                app.status =
                    "usage: /agent <id> | /agent result <id> | /agent switch <id> | /agent message <id> <message>".into();
                return;
            }
            match api.agent_run(&Id(arguments.into())).await {
                Ok(agent) => {
                    app.tool_result = agent_detail(&agent);
                    app.status = "Agent detail · /agent switch <id> · /agent message <id> <message> · /wait <id> [seconds] · /close-agent <id>".into();
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/follow-up" => {
            let arguments = command
                .strip_prefix("/follow-up")
                .map(str::trim)
                .unwrap_or_default();
            let Some((agent, content)) = arguments.split_once(char::is_whitespace) else {
                app.status = "usage: /follow-up <agent-id> <message>".into();
                return;
            };
            let content = content.trim();
            if content.is_empty() {
                app.status = "usage: /follow-up <agent-id> <message>".into();
                return;
            }
            match api
                .create_agent_follow_up(&Id(agent.into()), content.into())
                .await
            {
                Ok(task) => {
                    app.status = format!("follow-up Agent queued as {}", task.id.0);
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/interrupt" => {
            let id = command
                .strip_prefix("/interrupt")
                .map(str::trim)
                .unwrap_or_default();
            if id.is_empty() {
                app.status = "usage: /interrupt <agent-id>".into();
                return;
            }
            match api.control_durable_task(&Id(id.into()), "cancel").await {
                Ok(task) => {
                    app.status = format!("Agent {} interrupted", task.id.0);
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/wait" => {
            let arguments = command
                .strip_prefix("/wait")
                .map(str::trim)
                .unwrap_or_default();
            let mut parts = arguments.split_whitespace();
            let id = parts.next().unwrap_or_default();
            let timeout = parts
                .next()
                .map(str::parse::<u64>)
                .transpose()
                .ok()
                .flatten()
                .unwrap_or(30);
            if id.is_empty() || timeout > 60 || parts.next().is_some() {
                app.status = "usage: /wait <agent-id> [0-60 seconds]".into();
                return;
            }
            app.status = format!("waiting up to {timeout}s for Agent {id}");
            match api.wait_agent(&Id(id.into()), timeout).await {
                Ok(agent) => {
                    let terminal = agent_status_is_terminal(&agent.status);
                    app.tool_result = agent_detail(&agent);
                    app.status = if terminal {
                        format!("Agent {} finished with {:?}", agent.id.0, agent.status)
                    } else {
                        format!("Agent {} is still {:?}", agent.id.0, agent.status)
                    };
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/close-agent" => {
            let id = command
                .strip_prefix("/close-agent")
                .map(str::trim)
                .unwrap_or_default();
            if id.is_empty() {
                app.status = "usage: /close-agent <agent-id>".into();
                return;
            }
            match api.close_agent(&Id(id.into())).await {
                Ok(agent) => {
                    app.tool_result = agent_detail(&agent);
                    app.status = if agent_status_is_terminal(&agent.status) {
                        format!("Agent {} is closed", agent.id.0)
                    } else {
                        format!("Agent {} close requested", agent.id.0)
                    };
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/archive" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before archiving".into();
                return;
            }
            if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api.archive_session(&session_id).await {
                    Ok(updated) => {
                        app.sessions[app.selected] = updated;
                        app.status = "session archived; use /unarchive to continue".into();
                        app.turn_running = false;
                        app.pending_inputs.clear();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/unarchive" => {
            if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api
                    .update_session(&session_id, None, Some(SessionStatus::Active))
                    .await
                {
                    Ok(updated) => {
                        app.sessions[app.selected] = updated;
                        app.status = "session restored".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/delete" => {
            if !app.pending_attachments.is_empty() {
                app.status = "detach pending files before deleting the session".into();
                return;
            }
            let confirmation = command
                .strip_prefix("/delete")
                .map(str::trim)
                .unwrap_or_default();
            if let Some(session) = app.current().cloned() {
                if confirmation != session.id.0 {
                    app.status =
                        format!("permanent action: type /delete {} to confirm", session.id.0);
                } else {
                    match api.delete_session(&session.id).await {
                        Ok(_) => {
                            app.sessions.remove(app.selected);
                            app.selected = app.selected.min(app.sessions.len().saturating_sub(1));
                            app.pending_inputs.clear();
                            app.clear_transcript();
                            if let Some(next) = app.current().map(|session| session.id.clone())
                                && let Err(error) = load_session_state(api, app, &next).await
                            {
                                app.activity.push_front(format!("× {error}"));
                            }
                            app.status = "session permanently deleted".into();
                        }
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
            } else {
                app.status = "there is no session to delete".into();
            }
        }
        "/diff" => {
            if let Some(session) = app.current() {
                app.tool_result = api
                    .diff(&session.id)
                    .await
                    .map(|value| readable_tool_result(&value))
                    .unwrap_or_else(|error| format!("× {error}"));
                app.status = "changes loaded".into();
            }
        }
        "/raw" => {
            if let Some(answer) = last_assistant_text(app) {
                app.tool_result = answer;
                app.tool_result_expanded = false;
                app.status = "showing the raw last answer".into();
            } else {
                app.status = "there is no assistant answer to show".into();
            }
        }
        "/output" => {
            if app.tool_result.is_empty()
                || app.tool_result == "Press d to load the current Git diff."
            {
                app.status = "there is no result to expand".into();
            } else if app.tool_result.lines().count() <= 12 {
                app.status = "the current result already fits without folding".into();
            } else {
                app.tool_result_expanded = !app.tool_result_expanded;
                app.status = if app.tool_result_expanded {
                    "long result expanded · /output collapses it".into()
                } else {
                    "long result collapsed · /output expands it".into()
                };
            }
        }
        "/links" => {
            let mut seen = std::collections::HashSet::new();
            let options = app
                .messages
                .iter()
                .rev()
                .filter(|message| message.role == "assistant")
                .flat_map(|message| transcript_links(&value_text(&message.content)))
                .filter(|link| seen.insert(link.url.clone()))
                .take(200)
                .map(|link| PickerOption {
                    id: link.url.clone(),
                    label: link.label,
                    detail: link.url,
                    disabled_reason: None,
                })
                .collect::<Vec<_>>();
            if options.is_empty() {
                app.status = "there are no safe HTTP(S) links in assistant history".into();
            } else {
                app.picker = Some(PickerState {
                    kind: PickerKind::Link,
                    title: "Links · Enter opens in the system browser".into(),
                    options,
                    query: command
                        .strip_prefix("/links")
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_owned(),
                    selected: 0,
                });
                app.status = "link picker · type to search".into();
            }
        }
        "/usage" => {
            let usage = &app.usage;
            app.tool_result = format!(
                "Recorded session usage\n\nTokens       {}\nInput        {}\nOutput       {}\nModel calls  {}\nTool calls   {}\nTurns        {}\n\nProvider usage can be incomplete; see each task's usage note.",
                usage.total_tokens,
                usage.input_tokens,
                usage.output_tokens,
                usage.model_calls,
                usage.tool_calls,
                usage.turns,
            );
            app.status = format!(
                "session usage · {} tokens · {} turns",
                usage.total_tokens, usage.turns
            );
        }
        "/copy" => {
            if let Some(answer) = last_assistant_text(app) {
                match write_osc52(io::stdout(), &answer) {
                    Ok(()) => app.status = "last answer copied to the clipboard".into(),
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else {
                app.status = "there is no assistant answer to copy".into();
            }
        }
        "/editor" => {
            let initial = command
                .strip_prefix("/editor")
                .map(str::trim_start)
                .unwrap_or_default();
            app.pending_editor = Some(initial.to_owned());
            app.status = "opening external editor".into();
        }
        "/theme" => {
            let value = command
                .strip_prefix("/theme")
                .map(str::trim)
                .unwrap_or_default();
            if value.is_empty() {
                app.status = format!(
                    "theme: {} · usage: /theme <dark|light|no_color>",
                    app.theme.name()
                );
            } else if let Some(theme) = CliTheme::parse(value) {
                app.theme = theme;
                app.status = format!("theme: {}", theme.name());
            } else {
                app.status = "usage: /theme <dark|light|no_color>".into();
            }
        }
        "/statusline" => {
            let value = command
                .strip_prefix("/statusline")
                .map(str::trim)
                .unwrap_or_default();
            if value.is_empty() {
                app.status = format!(
                    "statusline: {} · usage: /statusline <full|compact|off>",
                    app.statusline.name()
                );
            } else if let Some(statusline) = StatuslineMode::parse(value) {
                app.statusline = statusline;
                app.status = format!("statusline: {}", statusline.name());
            } else {
                app.status = "usage: /statusline <full|compact|off>".into();
            }
        }
        "/keymap" | "/vim" => {
            let value = if command.starts_with("/vim") {
                if app.keymap == Keymap::Vim {
                    "emacs"
                } else {
                    "vim"
                }
            } else {
                command
                    .strip_prefix("/keymap")
                    .map(str::trim)
                    .unwrap_or_default()
            };
            if value.is_empty() {
                app.status = format!("keymap: {} · usage: /keymap <emacs|vim>", app.keymap.name());
            } else if let Some(keymap) = Keymap::parse(value) {
                app.keymap = keymap;
                app.vim_mode = if keymap == Keymap::Vim {
                    VimMode::Normal
                } else {
                    VimMode::Insert
                };
                app.status = format!("keymap: {}", keymap.name());
            } else {
                app.status = "usage: /keymap <emacs|vim>".into();
            }
        }
        "/context" => {
            if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api.context_summary(&session_id).await {
                    Ok(summary) => {
                        let mut lines = vec![
                            format!(
                                "Context: {} / {} estimated tokens",
                                summary.total_estimated_tokens, summary.max_input_tokens
                            ),
                            format!(
                                "Conversation: {} · Sources: {} · Reserved output: {}",
                                summary.conversation_tokens,
                                summary.item_tokens,
                                summary.reserved_output_tokens
                            ),
                        ];
                        lines.extend(summary.items.into_iter().map(|item| {
                            format!(
                                "{} · {} tokens · {} · {}",
                                item.kind, item.estimated_tokens, item.trust_level, item.source_uri
                            )
                        }));
                        app.tool_result = lines.join("\n");
                        app.status = "context sources loaded".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else {
                app.status = "select a session before inspecting context".into();
            }
        }
        "/compact" => {
            let focus = command
                .strip_prefix("/compact")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            if app.turn_running {
                app.status = "wait for the running turn before compacting context".into();
            } else if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api.compact_session(&session_id, focus).await {
                    Ok(result) => {
                        app.tool_result = format!(
                            "Context compacted\n{} → {} estimated tokens\n{} messages summarized · {} messages shortened{}",
                            result.before_tokens,
                            result.after_tokens,
                            result.omitted_messages,
                            result.truncated_messages,
                            if result.focus_applied {
                                "\nFocus saved as 24-hour Project memory"
                            } else {
                                ""
                            }
                        );
                        app.status = "context compacted; full transcript preserved".into();
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else {
                app.status = "select a session before compacting context".into();
            }
        }
        "/learn" => {
            let Some(session_id) = app.current().map(|session| session.id.clone()) else {
                app.status = "select a session before managing project learning".into();
                return;
            };
            let arguments = command.strip_prefix("/learn").unwrap_or_default().trim();
            let mode = match arguments {
                "on" => Some(s_code_protocol::LearningMode::Learn),
                "off" => Some(s_code_protocol::LearningMode::Off),
                "reuse" => Some(s_code_protocol::LearningMode::Reuse),
                _ => None,
            };
            if let Some(mode) = mode {
                match api.set_learning(&session_id, mode).await {
                    Ok(settings) => {
                        app.status = format!(
                            "Project learning: {:?}. Learning uses at most one extra model call after a verified task.",
                            settings.mode
                        )
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else if arguments == "clear" || arguments.starts_with("remove ") {
                let id = arguments.strip_prefix("remove ").map(str::trim);
                match api.forget_lessons(&session_id, id).await {
                    Ok(()) => {
                        app.status =
                            "Project experience removed; in-flight learning revoked.".into()
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            } else if arguments.is_empty() || arguments == "list" {
                match (
                    api.learning(&session_id).await,
                    api.lessons(&session_id).await,
                ) {
                    (Ok(settings), Ok(lessons)) => {
                        app.status = format!(
                            "Project learning: {:?} · {} lessons",
                            settings.mode,
                            lessons.len()
                        );
                        app.tool_result = lessons
                            .iter()
                            .map(|lesson| {
                                format!(
                                    "{} · {}\n{}\nSource: {} · expires {}",
                                    lesson.id.0,
                                    lesson.applicability,
                                    lesson.guidance,
                                    lesson.source_turn_id.0,
                                    lesson.expires_at
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if lessons.is_empty() {
                            app.tool_result = "No learned project experience. Use /learn on to learn from future tasks; /learn reuse to freeze learning.".into();
                        }
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        app.activity.push_front(format!("× {error}"))
                    }
                }
            } else {
                app.status = "usage: /learn [on|off|reuse|list|clear|remove <id>]".into();
            }
        }
        "/memory" => {
            let Some(session_id) = app.current().map(|session| session.id.clone()) else {
                app.status = "select a session before managing memory".into();
                return;
            };
            let arguments = command
                .strip_prefix("/memory")
                .map(str::trim)
                .unwrap_or_default();
            if arguments.is_empty() || arguments == "list" {
                match api.memories(&session_id).await {
                    Ok(memories) => {
                        app.tool_result = if memories.is_empty() {
                            "No active User, Project, or Team memory.".into()
                        } else {
                            memories
                                .iter()
                                .map(|memory| {
                                    format!(
                                        "{}  {:?}  {}\n  {}\n  {}",
                                        memory.id.0,
                                        memory.memory_scope,
                                        memory.citation,
                                        memory.content,
                                        memory.source_uri
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        };
                        app.status = format!("{} active memories", memories.len());
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
                return;
            }
            if let Some(id) = arguments.strip_prefix("remove ").map(str::trim) {
                if id.is_empty() {
                    app.status = "usage: /memory remove <memory-id>".into();
                } else {
                    match api.delete_memory(&Id(id.into())).await {
                        Ok(()) => app.status = format!("removed memory {id}"),
                        Err(error) => app.activity.push_front(format!("× {error}")),
                    }
                }
                return;
            }
            if arguments == "clear" {
                match api.memories(&session_id).await {
                    Ok(memories) => {
                        let mut removed = 0usize;
                        for memory in memories {
                            if api.delete_memory(&memory.id).await.is_ok() {
                                removed += 1;
                            }
                        }
                        app.status = format!("cleared {removed} memories visible to this session");
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
                return;
            }
            let Some(addition) = arguments.strip_prefix("add ") else {
                app.status = "usage: /memory [list|clear|remove <id>|add <user|project|team> <7d|30d|90d|never> <citation> :: <content>]".into();
                return;
            };
            let Some((metadata, content)) = addition.split_once("::") else {
                app.status = "memory add requires a citation followed by :: and content".into();
                return;
            };
            let mut metadata = metadata.splitn(3, char::is_whitespace);
            let memory_scope = match metadata.next().unwrap_or_default().trim() {
                "user" => MemoryScope::User,
                "project" => MemoryScope::Project,
                "team" => MemoryScope::Team,
                _ => {
                    app.status = "memory scope must be user, project, or team".into();
                    return;
                }
            };
            let expiry = metadata.next().unwrap_or_default().trim();
            let citation = metadata.next().unwrap_or_default().trim();
            let expires_at = if expiry == "never" {
                None
            } else if let Some(days) = expiry
                .strip_suffix('d')
                .and_then(|value| value.parse::<i64>().ok())
            {
                if !(1..=3650).contains(&days) {
                    app.status = "memory expiry must be between 1d and 3650d".into();
                    return;
                }
                Some(chrono::Utc::now() + chrono::Duration::days(days))
            } else {
                app.status = "memory expiry must be 7d, 30d, 90d, or never".into();
                return;
            };
            if citation.is_empty() || content.trim().is_empty() {
                app.status = "memory citation and content cannot be empty".into();
                return;
            }
            match api
                .create_memory(
                    &session_id,
                    memory_scope,
                    citation.into(),
                    content.trim().into(),
                    expires_at,
                )
                .await
            {
                Ok(memory) => {
                    app.tool_result = format!(
                        "{}\n{}\n{}",
                        memory.citation, memory.content, memory.source_uri
                    );
                    app.status = format!("saved {} memory", memory.id.0);
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/init" => {
            if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match api.workspace_paths(&session_id, "AGENTS.md", 20).await {
                    Ok(paths) if paths.iter().any(|path| path.path == "AGENTS.md") => {
                        app.status = "AGENTS.md already exists; no file was changed".into();
                    }
                    Ok(_) => {
                        match api
                            .submit_tool(
                                &session_id,
                                "apply_patch",
                                json!({
                                    "path": "AGENTS.md",
                                    "expected_sha256": null,
                                    "content": "# AGENTS.md\n\n## Project\n\nDescribe the project architecture and the commands agents should use.\n\n## Development\n\n- Keep changes focused and reversible.\n- Run the relevant tests before reporting completion.\n- Keep generated and temporary files out of source directories.\n"
                                }),
                            )
                            .await
                        {
                            Ok(outcome) => {
                                app.current_turn = outcome
                                    .pointer("/tool_call/request/turn_id")
                                    .and_then(Value::as_str)
                                    .map(|value| Id(value.into()));
                                app.turn_running = outcome["outcome"] == "awaiting_approval";
                                app.status = if app.turn_running {
                                    "AGENTS.md creation awaiting approval"
                                } else {
                                    "AGENTS.md initialized"
                                }
                                .into();
                            }
                            Err(error) => app.activity.push_front(format!("× {error}")),
                        }
                    }
                    Err(error) => app.activity.push_front(format!("× {error}")),
                }
            }
        }
        "/answer" => {
            let rest = command
                .strip_prefix("/answer")
                .map(str::trim)
                .unwrap_or_default();
            let Some((request_id, answer_text)) = rest.split_once(char::is_whitespace) else {
                app.status =
                    "usage: /answer <request-id> <answer> or question_id=answer;...".into();
                return;
            };
            let Some(pending) = app
                .questions
                .iter()
                .find(|question| {
                    question.id.0 == request_id && question.status == QuestionStatus::Pending
                })
                .cloned()
            else {
                app.status = format!("no pending question matches {request_id:?}");
                return;
            };
            let answers = if pending.questions.len() == 1 && !answer_text.contains('=') {
                vec![QuestionAnswer {
                    question_id: pending.questions[0].id.clone(),
                    answer: answer_text.trim().to_owned(),
                }]
            } else {
                let mut answers = Vec::new();
                for entry in answer_text.split(';') {
                    let Some((question_id, answer)) = entry.split_once('=') else {
                        app.status =
                            "multiple answers must use question_id=answer;question_id=answer"
                                .into();
                        return;
                    };
                    answers.push(QuestionAnswer {
                        question_id: question_id.trim().to_owned(),
                        answer: answer.trim().to_owned(),
                    });
                }
                answers
            };
            match api.answer_question(&pending.id, answers).await {
                Ok(answered) => {
                    if let Some(question) = app
                        .questions
                        .iter_mut()
                        .find(|question| question.id == answered.id)
                    {
                        *question = QuestionActivity::from(answered);
                    }
                    app.status = "answer submitted; continuing the turn".into();
                    app.turn_running = true;
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/artifact" => {
            let id = command
                .strip_prefix("/artifact")
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let Some(id) = id else {
                app.status = "usage: /artifact <artifact-id>".into();
                return;
            };
            match api.artifact(&Id(id.into())).await {
                Ok(artifact) => {
                    app.tool_result = if let Some(content) = artifact.content.as_str() {
                        content.to_owned()
                    } else {
                        serde_json::to_string_pretty(&artifact.content)
                            .unwrap_or_else(|_| value_text(&artifact.content))
                    };
                    app.status = format!("showing artifact · {}", artifact.metadata.title);
                }
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
        }
        "/undo" => {
            let requested_turn = command
                .strip_prefix("/undo")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Id(value.to_owned()));
            if !app.undo_enabled {
                app.tool_result = "Undo is unavailable for this service.".into();
            } else if let Some(turn) = if requested_turn.is_some() {
                requested_turn
            } else if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                match latest_meaningful_checkpoint(api, &session_id).await {
                    Ok(turn) => turn,
                    Err(error) => {
                        app.tool_result = format!("× {error}");
                        return;
                    }
                }
            } else {
                None
            } {
                match api.undo_turn(&turn).await {
                    Ok(value) => {
                        app.tool_result = readable_tool_result(&value);
                        if let Some(session_id) = app.current().map(|session| session.id.clone()) {
                            if let Err(error) = load_session_state(api, app, &session_id).await {
                                app.activity.push_front(format!("× {error}"));
                            }
                            load_session_goal(api, app, &session_id).await;
                        }
                        app.status = "turn and conversation state restored".into();
                    }
                    Err(error) => app.tool_result = format!("× {error}"),
                }
            } else {
                app.status = "there is no completed turn to restore".into();
            }
        }
        "/status" => {
            let session = app
                .current()
                .map(|session| format!("{} · {}", session.model, session.workspace_uri))
                .unwrap_or_else(|| "no session".into());
            app.activity.push_front(format!("✓ Connected · {session}"));
            app.status = "connected".into();
        }
        "/permissions" => match api.permission_profiles().await {
            Ok(profiles) => {
                app.picker = Some(PickerState {
                    kind: PickerKind::Permission,
                    title: "Choose a policy-bounded permission profile".into(),
                    options: profiles
                        .into_iter()
                        .map(|profile| PickerOption {
                            id: permission_mode_name(&profile.mode).into(),
                            label: profile.label,
                            detail: format!(
                                "{} · files: {} · commands: {}",
                                profile.description, profile.file_changes, profile.commands
                            ),
                            disabled_reason: profile.locked_reason,
                        })
                        .collect(),
                    query: command
                        .strip_prefix("/permissions")
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_owned(),
                    selected: 0,
                });
                app.status = "permission picker · type to search".into();
            }
            Err(error) => app.activity.push_front(format!("× {error}")),
        },
        other => {
            app.status = format!("unknown command {other}; type /help");
        }
    }
}

async fn latest_meaningful_checkpoint(api: &Api, session_id: &Id) -> Result<Option<Id>> {
    let turns = api.turns(session_id).await?;
    for turn in turns.iter().rev().filter(|turn| {
        matches!(
            turn.status,
            TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
        )
    }) {
        let impact = api.turn_undo_impact(&turn.id).await?;
        if checkpoint_has_impact(&impact) {
            return Ok(Some(turn.id.clone()));
        }
    }
    Ok(None)
}

fn checkpoint_has_impact(impact: &TurnUndoImpactPreview) -> bool {
    !impact.paths.is_empty()
        || impact.conversation_messages > 0
        || impact.plan_items > 0
        || impact.queued_inputs > 0
        || impact.session_goal_changes > 0
}

pub(crate) fn move_approval_selection(app: &mut App, direction: isize) {
    const APPROVAL_CHOICES: isize = 2;
    app.approval_selected =
        (app.approval_selected as isize + direction).rem_euclid(APPROVAL_CHOICES) as usize;
}

pub(crate) fn selected_approval_decision(app: &App) -> (bool, ApprovalScope) {
    match app.approval_selected.min(1) {
        0 => (true, ApprovalScope::Once),
        _ => (false, ApprovalScope::Once),
    }
}

pub(crate) async fn run_interactive_loop(
    api: &Api,
    app: &mut App,
    manifest: &s_code_protocol::CapabilityManifest,
) -> Result<()> {
    let (live_tx, mut live_rx) = mpsc::channel(256);
    tokio::spawn(stream_events(api.clone(), live_tx, app.event_cursor));
    let presence_client_id = format!("cli:{}", Id::new("client").0);
    let mut presence_tick = tokio::time::interval(Duration::from_secs(15));
    presence_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut guard = TerminalGuard::enter()?;
    let mut input_events = EventStream::new();
    loop {
        guard.terminal.draw(|frame| render(frame, app))?;
        tokio::select! {
            event = input_events.next() => {
                let Some(Ok(event)) = event else { continue };
                if let TerminalEvent::Paste(value) = event {
                    apply_bracketed_paste(app, &value);
                    continue;
                }
                let TerminalEvent::Key(key) = event else { continue };
                if key.kind != KeyEventKind::Press { continue; }
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if !app.input.is_empty() {
                        app.input.clear();
                        app.composer_input_changed();
                        app.status = "input cleared".into();
                    } else if let Some(turn) = app.current_turn.clone()
                        && app.turn_running
                    {
                        if let Err(error) = api.cancel_turn(&turn).await {
                            app.activity.push_front(format!("× {error}"));
                        } else {
                            app.status = "stopping".into();
                        }
                    } else {
                        break;
                    }
                    continue;
                }
                if let Some(approval) = app.approvals.front().cloned() {
                    let decision = match key.code {
                        KeyCode::Char('1') => Some((true, ApprovalScope::Once)),
                        KeyCode::Char('2') => Some((false, ApprovalScope::Once)),
                        KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                            move_approval_selection(app, -1);
                            continue;
                        }
                        KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                            move_approval_selection(app, 1);
                            continue;
                        }
                        KeyCode::Enter => Some(selected_approval_decision(app)),
                        KeyCode::Esc => {
                            app.approval_selected = 1;
                            continue;
                        }
                        _ => None,
                    };
                    if let Some((approved, scope)) = decision {
                        if let Err(error) = api.approval(&approval.id, approved, scope).await {
                            app.activity.push_front(format!("× {error}"));
                        } else {
                            app.approvals.pop_front();
                            app.approval_selected = 1;
                            app.resolve_tool_approval(&approval, approved);
                        }
                        continue;
                    }
                }
                if app.picker.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            app.picker = None;
                            app.status = "picker closed".into();
                        }
                        KeyCode::Up => {
                            if let Some(picker) = app.picker.as_mut() {
                                picker.move_selection(-1);
                            }
                        }
                        KeyCode::Down => {
                            if let Some(picker) = app.picker.as_mut() {
                                picker.move_selection(1);
                            }
                        }
                        KeyCode::Enter => apply_picker_selection(api, app).await,
                        KeyCode::Backspace => {
                            if let Some(picker) = app.picker.as_mut() {
                                picker.query.pop();
                                picker.selected = 0;
                            }
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(picker) = app.picker.as_mut() {
                                picker.query.clear();
                                picker.selected = 0;
                            }
                        }
                        KeyCode::Char(character)
                            if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            if let Some(picker) = app.picker.as_mut() {
                                picker.query.push(character);
                                picker.selected = 0;
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if slash_command_menu_visible(app) {
                    match key.code {
                        KeyCode::Up => {
                            move_slash_command_selection(app, -1);
                            continue;
                        }
                        KeyCode::Down => {
                            move_slash_command_selection(app, 1);
                            continue;
                        }
                        KeyCode::Tab => {
                            complete_slash_command(app);
                            continue;
                        }
                        KeyCode::Enter
                            if !key.modifiers.contains(KeyModifiers::SHIFT) =>
                        {
                            if !slash_command_input_is_exact(app) {
                                complete_slash_command(app);
                                continue;
                            }
                        }
                        KeyCode::Esc => {
                            app.slash_command_dismissed = true;
                            app.status = "slash command menu closed".into();
                            continue;
                        }
                        _ => {}
                    }
                }
                match key.code {
                    KeyCode::PageUp => {
                        if let (Some(cursor), Some(session_id)) = (
                            app.transcript_next_cursor.clone(),
                            app.current().map(|session| session.id.clone()),
                        ) {
                            app.status = "loading earlier transcript".into();
                            match api
                                .transcript_snapshot_page(&session_id, Some(&cursor), 250)
                                .await
                            {
                                Ok(snapshot) => merge_older_transcript_snapshot(app, snapshot),
                                Err(error) => app.activity.push_front(format!("× {error}")),
                            }
                        }
                        app.scroll_transcript_up(10);
                        app.status = if app.transcript_next_cursor.is_some() {
                            format!(
                                "viewing earlier transcript · {}/{} items loaded",
                                app.transcript_loaded_items, app.transcript_item_count
                            )
                        } else {
                            "viewing earlier transcript · all history loaded".into()
                        };
                        continue;
                    }
                    KeyCode::PageDown => {
                        app.scroll_transcript_down(10);
                        app.status = if app.transcript_scroll == 0 {
                            "following latest transcript".into()
                        } else {
                            "viewing earlier transcript".into()
                        };
                        continue;
                    }
                    KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let initial = app.input.as_str().to_owned();
                        open_external_editor(&mut guard, app, &initial);
                        continue;
                    }
                    _ => {}
                }
                if handle_vim_key(app, key.code) {
                    continue;
                }
                match key.code {
                    KeyCode::Tab if app.input_mode == InputMode::Prompt
                        && app.input.as_str().starts_with('@') =>
                    {
                        complete_file_mention(api, app).await;
                    }
                    KeyCode::Esc if app.input_mode != InputMode::Prompt => { app.input_mode = InputMode::Prompt; app.pending_workspace = None; app.input.clear(); app.composer_input_changed(); app.status = "session creation cancelled".into(); }
                    KeyCode::Esc => {
                        if let Some(turn) = app.current_turn.clone()
                            && app.turn_running
                        {
                            if let Err(error) = api.cancel_turn(&turn).await {
                                app.activity.push_front(format!("× {error}"));
                            } else {
                                app.status = "stopping".into();
                            }
                        } else {
                            app.status = "press Ctrl-D on an empty prompt to exit".into();
                        }
                    }
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => app.input.insert('\n'),
                    KeyCode::Enter if !app.input.trim().is_empty() => {
                        match app.input_mode {
                            InputMode::NewWorkspace => { app.pending_workspace = Some(app.input.take()); app.input_mode = InputMode::NewModel; app.status = "enter an approved model ID".into(); }
                            InputMode::NewModel => {
                                let model = app.input.take(); let workspace = app.pending_workspace.take().unwrap_or_default();
                                match api.create_session(workspace, model).await {
                                    Ok(session) => {
                                        let session_id = session.id.clone();
                                        app.sessions.push(session);
                                        app.selected = app.sessions.len() - 1;
                                        app.pending_inputs.clear();
                                        app.clear_transcript();
                                        app.input_mode = InputMode::Prompt;
                                        load_session_preferences(api, app, &session_id).await;
                                        load_session_goal(api, app, &session_id).await;
                                        app.status = "session created".into();
                                    }
                                    Err(error) => {
                                        app.activity.push_front(error.to_string());
                                        app.input_mode = InputMode::NewWorkspace;
                                    }
                                }
                            }
                            InputMode::Prompt => {
                                let prompt = app.input.take();
                                app.composer_input_changed();
                                if is_local_exit(&prompt) {
                                    break;
                                } else if prompt.starts_with('/') {
                                    run_command(api, app, &prompt).await;
                                    if let Some(initial) = app.pending_editor.take() {
                                        open_external_editor(&mut guard, app, &initial);
                                    }
                                } else if app.turn_running {
                                    submit_pending_input(
                                        api,
                                        app,
                                        prompt,
                                        TurnInputMode::Queue,
                                    )
                                    .await;
                                } else {
                                    start_prompt(api, app, prompt).await;
                                }
                            }
                        }
                    }
                    KeyCode::Left => app.input.move_left(),
                    KeyCode::Right => app.input.move_right(),
                    KeyCode::Home => app.input.move_line_start(),
                    KeyCode::End => app.input.move_line_end(),
                    KeyCode::Backspace => {
                        app.input.backspace();
                        app.composer_input_changed();
                    }
                    KeyCode::Delete => {
                        app.input.delete();
                        app.composer_input_changed();
                    }
                    KeyCode::Up => recall_history(app, true),
                    KeyCode::Down => recall_history(app, false),
                    KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.move_line_start(),
                    KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.move_line_end(),
                    KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.kill_to_line_end(); app.composer_input_changed(); }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.kill_to_line_start(); app.composer_input_changed(); }
                    KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.kill_previous_word(); app.composer_input_changed(); }
                    KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.paste_killed(),
                    KeyCode::Char('_') if key.modifiers.contains(KeyModifiers::CONTROL) => app.input.undo(),
                    KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.insert('\n'); app.composer_input_changed(); }
                    KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        open_history_search(app);
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) && app.input.is_empty() => break,
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.delete(); app.composer_input_changed(); }
                    KeyCode::Char('?') if app.input.is_empty() => {
                        app.status = "help: / commands · /attach files · @ paths · ! shell".into()
                    }
                    KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => { app.input.insert(character); app.composer_input_changed(); }
                    _ => {}
                }
            }
            Some(event) = live_rx.recv() => {
                let input_changed = event.kind.starts_with("turn.input.");
                let goal_changed = event.kind == "session.goal.changed";
                let preferences_changed = event.kind == "session.preferences.updated";
                let source_input = (event.kind == "turn.created")
                    .then(|| {
                        event
                            .payload
                            .get("source_input_id")
                            .and_then(Value::as_str)
                            .map(|value| Id(value.into()))
                    })
                    .flatten();
                let created_message = source_input.as_ref().and_then(|_| {
                    Some((
                        event.item_id.clone()?,
                        event.session_id.clone()?,
                        event.turn_id.clone()?,
                        event.timestamp,
                    ))
                });
                let terminal = matches!(
                    event.kind.as_str(),
                    "turn.completed" | "turn.failed" | "turn.cancelled"
                ) && app.current_turn.as_ref() == event.turn_id.as_ref();
                if !app.apply_event(event) {
                    if (app.status.starts_with("event gap")
                        || app.status.starts_with("transcript append gap"))
                        && let Some(session_id) = app.current().map(|session| session.id.clone())
                        && let Err(error) = load_session_state(api, app, &session_id).await
                    {
                        app.activity.push_front(format!("× transcript restore failed: {error}"));
                    }
                    continue;
                }
                if goal_changed
                    && let Some(session_id) = app.current().map(|session| session.id.clone())
                {
                    load_session_goal(api, app, &session_id).await;
                }
                if preferences_changed
                    && let Some(session_id) = app.current().map(|session| session.id.clone())
                {
                    load_session_preferences(api, app, &session_id).await;
                }
                if let (Some(input_id), Some((item_id, session_id, turn_id, created_at))) =
                    (source_input, created_message)
                    && let Ok(input) = api.turn_input(&input_id).await
                    && !app.messages.iter().any(|message| message.id == item_id)
                {
                    app.track_transcript_item(item_id.clone());
                    app.messages.push(Message {
                        id: item_id,
                        session_id,
                        turn_id,
                        role: "user".into(),
                        content: input.content,
                        created_at,
                    });
                }
                if terminal
                    && let Some(session_id) = app.current().map(|session| session.id.clone())
                {
                    let _ = load_session_state(api, app, &session_id).await;
                } else if input_changed
                    && let Some(session_id) = app.current().map(|session| session.id.clone())
                    && let Ok(inputs) = api.pending_turn_inputs(&session_id).await
                {
                    app.pending_inputs = inputs.into();
                }
            },
            _ = presence_tick.tick() => {
                if manifest.supports("client.presence.v1", 1) {
                    let _ = api
                        .update_client_presence(
                            &presence_client_id,
                            app.current().map(|session| session.id.clone()),
                            true,
                        )
                        .await;
                }
            },
            () = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
    }
    if manifest.supports("client.presence.v1", 1) {
        let _ = api.remove_client_presence(&presence_client_id, false).await;
    }
    Ok(())
}
pub(crate) async fn submit_pending_input(
    api: &Api,
    app: &mut App,
    prompt: String,
    mode: TurnInputMode,
) {
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "select a session before adding input".into();
        return;
    };
    let Some(turn_id) = app.current_turn.clone() else {
        app.status = "there is no running turn to steer or queue".into();
        return;
    };
    match api
        .submit_turn_input(&session_id, &turn_id, mode.clone(), prompt)
        .await
    {
        Ok(input) => {
            if !app
                .pending_inputs
                .iter()
                .any(|candidate| candidate.id == input.id)
            {
                app.pending_inputs.push_back(input);
            }
            app.status = match mode {
                TurnInputMode::Steer => {
                    "steering current turn; the correction will continue automatically".into()
                }
                TurnInputMode::Queue => format!(
                    "{} message{} durably queued",
                    app.pending_inputs.len(),
                    if app.pending_inputs.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            };
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) fn handle_vim_key(app: &mut App, key: KeyCode) -> bool {
    if app.keymap != Keymap::Vim {
        return false;
    }
    if app.vim_mode == VimMode::Insert {
        if key == KeyCode::Esc {
            app.vim_mode = VimMode::Normal;
            app.status = "vim NORMAL · i insert · : command".into();
            return true;
        }
        return false;
    }
    if key == KeyCode::Enter {
        return false;
    }
    match key {
        KeyCode::Char('i') => app.vim_mode = VimMode::Insert,
        KeyCode::Char('a') => {
            app.input.move_right();
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('I') => {
            app.input.move_line_start();
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('A') => {
            app.input.move_line_end();
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Char('h') | KeyCode::Left => app.input.move_left(),
        KeyCode::Char('l') | KeyCode::Right => app.input.move_right(),
        KeyCode::Char('0') | KeyCode::Home => app.input.move_line_start(),
        KeyCode::Char('$') | KeyCode::End => app.input.move_line_end(),
        KeyCode::Char('x') | KeyCode::Delete => app.input.delete(),
        KeyCode::Char('u') => app.input.undo(),
        KeyCode::Char(':') => {
            app.input.replace("/");
            app.composer_input_changed();
            app.vim_mode = VimMode::Insert;
        }
        KeyCode::Esc => {}
        _ => {
            app.status = "vim NORMAL · i/a insert · h/l move · x delete · : command".into();
        }
    }
    true
}

pub(crate) async fn start_prompt(api: &Api, app: &mut App, prompt: String) {
    if let Some(command) = prompt.strip_prefix('!') {
        run_shell_command(api, app, command.trim()).await;
        return;
    }
    if !app.agent_enabled {
        app.status = "agent capability unavailable".into();
        return;
    }
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "create a session before starting a task".into();
        return;
    };
    app.prompt_history.push(prompt.clone());
    app.history_cursor = None;
    app.transcript_scroll = 0;
    app.status = "starting".into();
    let attachment_ids = app
        .pending_attachments
        .iter()
        .map(|attachment| attachment.id.clone())
        .collect();
    match api
        .start_turn(&session_id, prompt, attachment_ids, true)
        .await
    {
        Ok(turn) => {
            app.pending_attachments.clear();
            let _ = load_session_state(api, app, &session_id).await;
            app.current_turn = Some(turn);
            app.turn_running = true;
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) fn attachment_media_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "rs" | "go" | "py" | "js" | "ts" | "tsx" | "jsx" | "css" | "html"
        | "toml" | "yaml" | "yml" | "csv" | "log" => "text/plain",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

pub(crate) fn permission_mode_name(mode: &PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Manual => "manual",
        PermissionMode::AcceptEdits => "accept edits",
        PermissionMode::Workspace => "workspace",
        PermissionMode::Plan => "plan",
    }
}

pub(crate) fn parse_permission_mode(value: &str) -> Option<PermissionMode> {
    match value {
        "manual" => Some(PermissionMode::Manual),
        "accept-edits" | "accept_edits" | "accept edits" => Some(PermissionMode::AcceptEdits),
        "workspace" => Some(PermissionMode::Workspace),
        "plan" => Some(PermissionMode::Plan),
        _ => None,
    }
}

pub(crate) async fn load_session_preferences(api: &Api, app: &mut App, session_id: &Id) {
    match api.session_preferences(session_id).await {
        Ok(preferences) => {
            app.permission_mode = preferences.permission_mode;
            app.assistant_alias = preferences.assistant_alias;
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) async fn load_session_goal(api: &Api, app: &mut App, session_id: &Id) {
    match api.session_goal(session_id).await {
        Ok(goal) => app.goal = goal,
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) async fn apply_picker_selection(api: &Api, app: &mut App) {
    let Some(picker) = app.picker.take() else {
        return;
    };
    let Some(option) = picker.selected_option().cloned() else {
        app.status = "no picker option matches the current search".into();
        return;
    };
    if let Some(reason) = option.disabled_reason {
        app.status = reason;
        app.picker = Some(picker);
        return;
    }
    match picker.kind {
        PickerKind::History => {
            app.input.replace(&option.id);
            app.history_cursor = None;
            app.status = "history entry restored".into();
            return;
        }
        PickerKind::Link => {
            match open_external_url(&option.id) {
                Ok(_) => app.status = "external link opened".into(),
                Err(error) => app.activity.push_front(format!("× {error}")),
            }
            return;
        }
        PickerKind::Model | PickerKind::Permission => {}
    }
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "select a session before changing composer context".into();
        return;
    };
    match picker.kind {
        PickerKind::History | PickerKind::Link => {
            unreachable!("local picker selection returns before API dispatch")
        }
        PickerKind::Model => match api.update_session_model(&session_id, option.id).await {
            Ok(updated) => {
                if let Some(session) = app
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == updated.id)
                {
                    *session = updated.clone();
                }
                app.status = format!("model: {}", updated.model);
            }
            Err(error) => {
                app.activity.push_front(format!("× {error}"));
                app.picker = Some(picker);
            }
        },
        PickerKind::Permission => {
            let Some(mode) = parse_permission_mode(&option.id) else {
                app.status = "server returned an unknown permission profile".into();
                return;
            };
            match api
                .update_session_preferences(
                    &session_id,
                    UpdateSessionPreferences {
                        scope: api.scope.clone(),
                        permission_mode: Some(mode),
                        assistant_alias: None,
                    },
                )
                .await
            {
                Ok(preferences) => {
                    app.permission_mode = preferences.permission_mode;
                    app.status = format!(
                        "permission mode: {} · server policy still applies",
                        permission_mode_name(&app.permission_mode)
                    );
                }
                Err(error) => {
                    app.activity.push_front(format!("× {error}"));
                    app.picker = Some(picker);
                }
            }
        }
    }
}

pub(crate) fn agent_status_is_terminal(status: &DurableTaskStatus) -> bool {
    matches!(
        status,
        DurableTaskStatus::Succeeded | DurableTaskStatus::Failed | DurableTaskStatus::Cancelled
    )
}

pub(crate) fn agent_tree(agents: &[AgentRunSummary], parent_filter: Option<&Id>) -> String {
    let visible = agents
        .iter()
        .filter(|agent| parent_filter.is_none_or(|parent| agent.parent_id.as_ref() == Some(parent)))
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return if parent_filter.is_some() {
            "No matching subagents.".into()
        } else {
            "No Agent runs.".into()
        };
    }
    let by_id = agents
        .iter()
        .map(|agent| (agent.id.clone(), agent))
        .collect::<std::collections::HashMap<_, _>>();
    visible
        .iter()
        .take(30)
        .map(|agent| {
            let mut depth = 0;
            let mut parent = agent.parent_id.as_ref();
            let mut seen = std::collections::HashSet::new();
            while parent_filter.is_none()
                && let Some(id) = parent
                && depth < 8
                && seen.insert(id.clone())
            {
                depth += 1;
                parent = by_id.get(id).and_then(|parent| parent.parent_id.as_ref());
            }
            format!(
                "{}{}  {:?}{}  attempt {}  ${:.4}",
                "  ".repeat(depth),
                agent.id.0,
                agent.status,
                if agent.cancel_requested {
                    " · stopping"
                } else {
                    ""
                },
                agent.attempt,
                agent.consumed_cost_micros as f64 / 1_000_000.0
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_agent_results(results: &[AgentResultSummary]) -> String {
    let completed = results
        .iter()
        .take(30)
        .map(|result| {
            let summary = result
                .summary
                .as_deref()
                .unwrap_or("No final assistant message.")
                .lines()
                .map(|line| format!("  {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            let truncation = if result.truncated {
                format!(" · truncated from {} bytes", result.summary_byte_length)
            } else {
                String::new()
            };
            format!(
                "{}  {:?}  session {}  model ${:.4} · runner ${:.4} · {} artifact(s){}\n{}",
                result.agent_id.0,
                result.status,
                result.session_id.0,
                result.consumed_cost_micros as f64 / 1_000_000.0,
                result.consumed_runner_cost_micros as f64 / 1_000_000.0,
                result.artifact_count,
                truncation,
                summary,
            )
        })
        .collect::<Vec<_>>();
    if completed.is_empty() {
        "No completed Agent results.".into()
    } else {
        completed.join("\n")
    }
}

pub(crate) async fn switch_to_agent_session(api: &Api, app: &mut App, agent_id: &Id) {
    if !app.pending_attachments.is_empty() {
        app.status = "detach pending files before switching Agent Sessions".into();
        return;
    }
    let agent = match api.agent_run(agent_id).await {
        Ok(agent) => agent,
        Err(error) => {
            app.activity.push_front(format!("× {error}"));
            return;
        }
    };
    let Some(session_id) = agent.session_id else {
        app.status = format!("Agent {} has no Session to open", agent.id.0);
        return;
    };
    let sessions = match api.sessions().await {
        Ok(sessions) => sessions,
        Err(error) => {
            app.activity.push_front(format!("× {error}"));
            return;
        }
    };
    let Some(index) = sessions
        .iter()
        .position(|candidate| candidate.id == session_id)
    else {
        app.status = format!(
            "Agent Session {} is unavailable in this scope",
            session_id.0
        );
        return;
    };
    app.sessions = sessions;
    app.selected = index;
    if let Err(error) = load_session_state(api, app, &session_id).await {
        app.activity.push_front(format!("× {error}"));
        return;
    }
    load_session_preferences(api, app, &session_id).await;
    load_session_goal(api, app, &session_id).await;
    app.status = format!(
        "Viewing Agent {} · Session {} · use /resume to return through the Session picker",
        agent.id.0, session_id.0
    );
}

pub(crate) async fn send_agent_input(api: &Api, app: &mut App, agent_id: &Id, content: String) {
    let agent = match api.agent_run(agent_id).await {
        Ok(agent) => agent,
        Err(error) => {
            app.activity.push_front(format!("× {error}"));
            return;
        }
    };
    if agent_status_is_terminal(&agent.status) {
        app.status = format!(
            "Agent {} has finished; use /follow-up {} <message>",
            agent.id.0, agent.id.0
        );
        return;
    }
    let Some(session_id) = agent.session_id else {
        app.status = format!("Agent {} has no Session for direct input", agent.id.0);
        return;
    };
    let snapshot = match api.transcript_snapshot(&session_id).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            app.activity.push_front(format!("× {error}"));
            return;
        }
    };
    let Some(turn) = snapshot.turns.iter().rev().find(|turn| {
        !matches!(
            turn.status,
            TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
        )
    }) else {
        app.status = format!(
            "Agent {} has no active Turn; use /follow-up {} <message>",
            agent.id.0, agent.id.0
        );
        return;
    };
    match api
        .submit_turn_input(&session_id, &turn.id, TurnInputMode::Queue, content)
        .await
    {
        Ok(input) => {
            app.status = format!(
                "Message queued for Agent {} as input {}",
                agent.id.0, input.id.0
            );
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) fn agent_detail(agent: &AgentRunSummary) -> String {
    [
        format!("Agent: {}", agent.id.0),
        format!("Status: {:?}", agent.status),
        format!(
            "Parent: {}",
            agent
                .parent_id
                .as_ref()
                .map(|id| id.0.as_str())
                .unwrap_or("root")
        ),
        format!(
            "Session: {}",
            agent
                .session_id
                .as_ref()
                .map(|id| id.0.as_str())
                .unwrap_or("none")
        ),
        format!(
            "Goal: {}",
            agent
                .goal_id
                .as_ref()
                .map(|id| id.0.as_str())
                .unwrap_or("none")
        ),
        format!(
            "Team task: {}",
            agent
                .team_task_id
                .as_ref()
                .map(|id| id.0.as_str())
                .unwrap_or("none")
        ),
        format!(
            "Model: {}",
            agent.model.as_deref().unwrap_or("configured model")
        ),
        format!("Attempt: {}", agent.attempt),
        format!(
            "Cost: model ${:.4} · runner ${:.4}",
            agent.consumed_cost_micros as f64 / 1_000_000.0,
            agent.consumed_runner_cost_micros as f64 / 1_000_000.0
        ),
        format!(
            "Cancellation: {}",
            if agent.cancel_requested {
                "requested"
            } else {
                "not requested"
            }
        ),
        format!("Updated: {}", agent.updated_at),
    ]
    .join("\n")
}

pub(crate) const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;
const MAX_EDITOR_BYTES: u64 = 1024 * 1024;

pub(crate) fn last_assistant_text(app: &App) -> Option<String> {
    app.messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| value_text(&message.content))
}

pub(crate) fn write_osc52(mut writer: impl Write, text: &str) -> Result<()> {
    if text.is_empty() {
        return Err(anyhow!("the last assistant answer is empty"));
    }
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err(anyhow!(
            "the last assistant answer exceeds the 100 KiB clipboard limit"
        ));
    }
    let encoded = STANDARD.encode(text.as_bytes());
    writer.write_all(b"\x1b]52;c;")?;
    writer.write_all(encoded.as_bytes())?;
    writer.write_all(b"\x07")?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn external_editor_command(configured: Option<&str>) -> Option<String> {
    configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            env::var("VISUAL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

pub(crate) fn editor_runtime_directory() -> PathBuf {
    env::var_os("S_CODE_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("s-code-editor"))
}

pub(crate) fn edit_with_external_editor(
    command: &str,
    initial: &str,
    directory: Option<&Path>,
) -> Result<String> {
    let arguments = shell_words::split(command).context("external editor command is invalid")?;
    let (program, editor_arguments) = arguments
        .split_first()
        .context("external editor command is empty")?;
    let directory = directory
        .map(Path::to_path_buf)
        .unwrap_or_else(editor_runtime_directory);
    fs::create_dir_all(&directory)
        .with_context(|| format!("cannot create editor directory {}", directory.display()))?;
    let metadata = fs::symlink_metadata(&directory)
        .with_context(|| format!("cannot inspect editor directory {}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(anyhow!(
            "external editor directory must be a real directory"
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut path = None;
    for attempt in 0..16_u8 {
        let candidate =
            directory.join(format!("draft-{}-{nonce}-{attempt}.md", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(mut file) => {
                file.write_all(initial.as_bytes())?;
                file.sync_all()?;
                path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let path = path.context("could not allocate an external editor draft")?;
    let result = (|| -> Result<String> {
        let status = Command::new(program)
            .args(editor_arguments)
            .arg(&path)
            .status()
            .with_context(|| format!("cannot start external editor {program:?}"))?;
        if !status.success() {
            return Err(anyhow!("external editor exited with {status}"));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(anyhow!(
                "external editor replaced the draft with an unsafe file"
            ));
        }
        if metadata.len() > MAX_EDITOR_BYTES {
            return Err(anyhow!("external editor draft exceeds 1 MiB"));
        }
        let bytes = fs::read(&path)?;
        String::from_utf8(bytes).context("external editor draft is not UTF-8")
    })();
    let cleanup = fs::remove_file(&path);
    if result.is_ok() {
        cleanup.with_context(|| format!("cannot remove editor draft {}", path.display()))?;
    }
    result
}

pub(crate) fn open_external_editor(guard: &mut TerminalGuard, app: &mut App, initial: &str) {
    let Some(command) = external_editor_command(app.editor.as_deref()) else {
        app.status =
            "no editor configured; set client.editor, S_CODE_EDITOR, VISUAL, or EDITOR".into();
        return;
    };
    if let Err(error) = guard.suspend() {
        app.activity
            .push_front(format!("× cannot suspend terminal: {error}"));
        return;
    }
    let edited = edit_with_external_editor(&command, initial, None);
    let resumed = guard.resume();
    if let Err(error) = resumed {
        app.activity
            .push_front(format!("× cannot resume terminal: {error}"));
        return;
    }
    match edited {
        Ok(value) => {
            app.input.replace(&value);
            app.vim_mode = VimMode::Insert;
            app.status = "draft loaded from external editor".into();
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) async fn run_shell_command(api: &Api, app: &mut App, command: &str) {
    if command.is_empty() {
        app.status = "usage: ! <shell command>".into();
        return;
    }
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "create a session before running a command".into();
        return;
    };
    app.status = "submitting command for policy review".into();
    match api
        .submit_tool(
            &session_id,
            "run_command",
            json!({
                "program": "sh",
                "args": ["-lc", command],
                "timeout_seconds": 60,
                "network_enabled": false,
                "max_bytes": 1048576
            }),
        )
        .await
    {
        Ok(outcome) => {
            app.current_turn = outcome
                .pointer("/tool_call/request/turn_id")
                .and_then(Value::as_str)
                .map(|value| Id(value.into()));
            match outcome["outcome"].as_str() {
                Some("completed") => {
                    app.turn_running = false;
                    if let Ok(result) = completed_tool_result(outcome) {
                        app.tool_result = readable_tool_result(&result);
                    }
                    app.status = "command completed".into();
                }
                Some("awaiting_approval") => {
                    app.turn_running = true;
                    app.status = "command awaiting approval".into();
                }
                Some(other) => {
                    app.turn_running = false;
                    app.status = format!("command {other}");
                }
                None => app.activity.push_front("× invalid tool response".into()),
            }
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) async fn complete_file_mention(api: &Api, app: &mut App) {
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "create a session before searching files".into();
        return;
    };
    let query = app.input.as_str().trim().trim_start_matches('@').trim();
    match api.workspace_paths(&session_id, query, 8).await {
        Ok(paths) if paths.is_empty() => app.status = format!("no files match @{query}"),
        Ok(paths) => {
            app.input.replace(&format!("@{} ", paths[0].path));
            let alternatives = paths
                .iter()
                .take(4)
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            app.status = format!("file mention · {alternatives}");
        }
        Err(error) => app.activity.push_front(format!("× {error}")),
    }
}

pub(crate) fn readable_tool_result(value: &Value) -> String {
    if let Some(diff) = value.get("unified_diff").and_then(Value::as_str) {
        return diff.to_owned();
    }
    if let Some(paths) = value.get("restored_paths").and_then(Value::as_array) {
        let names = paths
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        return format!("Restored: {names}");
    }
    if let Some(object) = value.as_object() {
        return object
            .iter()
            .filter_map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| format!("{}: {value}", key.replace('_', " ")))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    value_text(value)
}

pub(crate) async fn stream_events(api: Api, sender: mpsc::Sender<LiveEvent>, mut after: u64) {
    loop {
        let path = format!(
            "/v1/events?organization_id={}&team_id={}&actor_id={}&after={after}",
            encode(&api.scope.organization_id.0),
            encode(&api.scope.team_id.0),
            encode(&api.scope.actor_id.0),
        );
        let response = api.send(api.request(reqwest::Method::GET, &path)).await;
        if let Ok(response) = response {
            let mut bytes = response.bytes_stream();
            let mut buffer = Vec::new();
            while let Some(Ok(chunk)) = bytes.next().await {
                buffer.extend_from_slice(&chunk);
                let Ok(events) = drain_sse(&mut buffer) else {
                    break;
                };
                for event in events {
                    if !accepts_stream_event(after, event.id) {
                        continue;
                    }
                    after = after.max(event.id);
                    if sender.send(event).await.is_err() {
                        return;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn accepts_stream_event(after: u64, event_id: u64) -> bool {
    event_id > after
}

#[cfg(test)]
#[test]
fn filtered_stream_cursor_accepts_monotonic_sequence_jumps() {
    assert!(accepts_stream_event(7, 9));
    assert!(!accepts_stream_event(9, 9));
    assert!(!accepts_stream_event(9, 8));
}

pub(crate) fn drain_sse(buffer: &mut Vec<u8>) -> Result<Vec<LiveEvent>, String> {
    const MAX_EVENT_BUFFER_BYTES: usize = 1024 * 1024;
    if buffer.len() > MAX_EVENT_BUFFER_BYTES {
        return Err("event stream frame exceeds 1 MiB".into());
    }
    let mut events = Vec::new();
    while let Some(boundary) = buffer.windows(2).position(|bytes| bytes == b"\n\n") {
        let block = buffer.drain(..boundary + 2).collect::<Vec<_>>();
        let block = std::str::from_utf8(&block[..block.len() - 2])
            .map_err(|_| "event stream contains invalid UTF-8".to_owned())?;
        let mut id = 0;
        let mut kind = "message".to_owned();
        let mut data = String::new();
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("id:") {
                id = value.trim().parse().unwrap_or(0);
            }
            if let Some(value) = line.strip_prefix("event:") {
                kind = value.trim().into();
            }
            if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.trim());
            }
        }
        if let Ok(envelope) = serde_json::from_str::<ClientEvent>(&data) {
            events.push(LiveEvent {
                id: id.max(envelope.sequence),
                timestamp: envelope.timestamp,
                kind: if envelope.kind.is_empty() {
                    kind
                } else {
                    envelope.kind
                },
                payload: envelope.payload,
                session_id: envelope.session_id,
                turn_id: envelope.turn_id,
                item_id: envelope.item_id,
            });
        }
    }
    Ok(events)
}

pub(crate) fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}
pub(crate) struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    fn suspend(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableBracketedPaste
        )?;
        self.terminal.clear()?;
        Ok(())
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
