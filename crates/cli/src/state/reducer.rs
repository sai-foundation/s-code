use super::{
    App, ApprovalRequest, ArtifactActivity, NoticeActivity, PlanActivity, QuestionActivity,
    ToolActivity, ToolActivityState, ToolProgress,
};
use opencoding_protocol::{
    Message, TranscriptItemContent, TranscriptItemStatus, TranscriptSnapshot, TurnStatus,
};

pub(crate) fn apply_transcript_snapshot(app: &mut App, snapshot: TranscriptSnapshot) {
    app.event_cursor = app.event_cursor.max(snapshot.cursor);
    app.clear_transcript();
    app.usage = snapshot.usage;
    app.transcript_loaded_items = u64::try_from(snapshot.items.len()).unwrap_or(u64::MAX);
    app.transcript_item_count = snapshot.item_count;
    app.transcript_next_cursor = snapshot.next_cursor.clone();

    for item in snapshot.items {
        app.track_transcript_item(item.id.clone());
        match item.content {
            TranscriptItemContent::Message {
                role,
                content,
                attachments,
            } => {
                if !attachments.is_empty() {
                    app.message_attachments.insert(item.id.clone(), attachments);
                }
                app.messages.push(Message {
                    id: item.id,
                    session_id: item.session_id,
                    turn_id: item.turn_id,
                    role,
                    content,
                    created_at: item.created_at,
                });
            }
            TranscriptItemContent::ToolCall {
                tool_call_id,
                tool,
                display,
                ..
            } => {
                let state = match item.status {
                    TranscriptItemStatus::Pending => ToolActivityState::Preparing,
                    TranscriptItemStatus::Started | TranscriptItemStatus::Streaming => {
                        ToolActivityState::Running
                    }
                    TranscriptItemStatus::AwaitingApproval
                    | TranscriptItemStatus::AwaitingInput => ToolActivityState::AwaitingApproval,
                    TranscriptItemStatus::Completed => ToolActivityState::Completed,
                    TranscriptItemStatus::Denied => ToolActivityState::Denied,
                    TranscriptItemStatus::Failed | TranscriptItemStatus::Cancelled => {
                        ToolActivityState::Failed
                    }
                };
                app.tool_activity.push(ToolActivity {
                    item_id: item.id,
                    turn_id: Some(item.turn_id),
                    call_id: Some(tool_call_id.0),
                    tool,
                    display,
                    state,
                    progress: None,
                });
            }
            TranscriptItemContent::McpCall {
                tool_call_id,
                namespaced_tool: tool,
                display,
                progress,
                ..
            } => {
                let state = match item.status {
                    TranscriptItemStatus::Pending => ToolActivityState::Preparing,
                    TranscriptItemStatus::Started | TranscriptItemStatus::Streaming => {
                        ToolActivityState::Running
                    }
                    TranscriptItemStatus::AwaitingApproval
                    | TranscriptItemStatus::AwaitingInput => ToolActivityState::AwaitingApproval,
                    TranscriptItemStatus::Completed => ToolActivityState::Completed,
                    TranscriptItemStatus::Denied => ToolActivityState::Denied,
                    TranscriptItemStatus::Failed | TranscriptItemStatus::Cancelled => {
                        ToolActivityState::Failed
                    }
                };
                app.tool_activity.push(ToolActivity {
                    item_id: item.id,
                    turn_id: Some(item.turn_id),
                    call_id: Some(tool_call_id.0),
                    tool,
                    display,
                    state,
                    progress: progress.map(|progress| ToolProgress {
                        progress: progress.progress,
                        total: progress.total,
                        message: progress.message,
                    }),
                });
            }
            TranscriptItemContent::Plan { title, steps } => {
                app.plans.push(PlanActivity {
                    id: item.id,
                    turn_id: item.turn_id,
                    title,
                    steps,
                });
            }
            TranscriptItemContent::ReasoningSummary { text } => {
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Reasoning summary".into(),
                    detail: text,
                });
            }
            TranscriptItemContent::Warning { code, message } => {
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: format!("Warning · {code}"),
                    detail: message,
                });
            }
            TranscriptItemContent::ContextCompaction {
                omitted_messages,
                truncated_messages,
                estimated_tokens,
            } => {
                let mut parts = vec![format!(
                    "{omitted_messages} earlier message{} summarized",
                    if omitted_messages == 1 { "" } else { "s" }
                )];
                if truncated_messages > 0 {
                    parts.push(format!(
                        "{truncated_messages} long message{} shortened",
                        if truncated_messages == 1 { "" } else { "s" }
                    ));
                }
                if estimated_tokens > 0 {
                    parts.push(format!("about {estimated_tokens} summary tokens"));
                }
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Context optimized".into(),
                    detail: parts.join(" · "),
                });
            }
            TranscriptItemContent::ModelReroute {
                from_model,
                to_model,
                reason,
            } => {
                let route = from_model
                    .map_or_else(|| to_model.clone(), |from| format!("{from} → {to_model}"));
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Model switched".into(),
                    detail: format!("{route} · {reason}"),
                });
            }
            TranscriptItemContent::Usage {
                model,
                input_tokens,
                output_tokens,
                total_tokens,
                model_calls,
                tool_calls,
            } => {
                app.usage_turns.insert(item.turn_id.clone());
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Usage".into(),
                    detail: format!(
                        "{total_tokens} tokens · {input_tokens} input + {output_tokens} output · \
                         {model_calls} model / {tool_calls} tool calls · {model}"
                    ),
                });
            }
            TranscriptItemContent::AgentStatus { label, .. } => {
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Agent".into(),
                    detail: label,
                });
            }
            TranscriptItemContent::Hook {
                event,
                handler,
                input_modified,
                result_summary,
                ..
            } => {
                let mut detail = format!("{event} · {handler}");
                if input_modified {
                    detail.push_str(" · input modified");
                }
                if let Some(summary) = result_summary {
                    detail.push_str(&format!(" · {summary}"));
                }
                app.notices.push(NoticeActivity {
                    item_id: item.id,
                    turn_id: item.turn_id,
                    label: "Hook".into(),
                    detail,
                });
            }
            TranscriptItemContent::Question { request } => {
                app.questions.push(QuestionActivity::from(*request));
            }
            TranscriptItemContent::Artifact {
                artifact_id,
                media_type,
                title,
            } => {
                app.artifacts.push(ArtifactActivity {
                    item_id: item.id,
                    artifact_id,
                    turn_id: item.turn_id,
                    title,
                    media_type,
                });
            }
            _ => {}
        }
    }
    for request in snapshot.pending_requests {
        app.approvals.push_back(ApprovalRequest {
            id: request.id.0,
            turn_id: Some(request.turn_id),
            tool: request.tool,
            display: request.summary,
        });
    }
    app.pending_inputs = snapshot.pending_inputs.into();
    if let Some(turn) = snapshot.turns.last() {
        app.current_turn = Some(turn.id.clone());
        app.turn_running = !matches!(
            turn.status,
            TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled
        );
        app.status = format!("{:?}", turn.status).to_ascii_lowercase();
    } else {
        app.current_turn = None;
        app.turn_running = false;
    }
}

pub(crate) fn merge_older_transcript_snapshot(app: &mut App, snapshot: TranscriptSnapshot) {
    let event_cursor = app.event_cursor;
    let usage = app.usage.clone();
    let usage_turns = app.usage_turns.clone();
    let current_turn = app.current_turn.clone();
    let turn_running = app.turn_running;
    let approvals = std::mem::take(&mut app.approvals);
    let pending_inputs = std::mem::take(&mut app.pending_inputs);
    let loaded_items = app.transcript_loaded_items;
    let item_count = app.transcript_item_count;

    let mut older = App::new(Vec::new(), app.agent_enabled, app.undo_enabled);
    apply_transcript_snapshot(&mut older, snapshot);

    older.messages.append(&mut app.messages);
    for item_id in std::mem::take(&mut app.transcript_item_order) {
        older.track_transcript_item(item_id);
    }
    older
        .message_attachments
        .extend(std::mem::take(&mut app.message_attachments));
    older.tool_activity.append(&mut app.tool_activity);
    older.plans.append(&mut app.plans);
    older.notices.append(&mut app.notices);
    older.questions.append(&mut app.questions);
    older.artifacts.append(&mut app.artifacts);

    app.messages = older.messages;
    app.transcript_item_order = older.transcript_item_order;
    app.message_attachments = older.message_attachments;
    app.tool_activity = older.tool_activity;
    app.plans = older.plans;
    app.notices = older.notices;
    app.questions = older.questions;
    app.artifacts = older.artifacts;
    app.approvals = approvals;
    app.pending_inputs = pending_inputs;
    app.event_cursor = event_cursor;
    app.usage = usage;
    app.usage_turns = usage_turns;
    app.current_turn = current_turn;
    app.turn_running = turn_running;
    app.transcript_next_cursor = older.transcript_next_cursor;
    app.transcript_loaded_items = loaded_items
        .saturating_add(older.transcript_loaded_items)
        .min(older.transcript_item_count.max(item_count));
    app.transcript_item_count = older.transcript_item_count.max(item_count);
}
