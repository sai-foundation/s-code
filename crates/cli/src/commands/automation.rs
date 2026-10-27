use crate::{
    CliExitStatus,
    api::{Api, encode},
    args::OutputMode,
    commands::interactive::{drain_sse, value_text},
    state::LiveEvent,
};
use anyhow::{Result, anyhow};
use futures_util::StreamExt;
use s_code_protocol::{ApprovalScope, Id};
use serde_json::{Value, json};
use std::io::{self, Write};

pub(crate) fn validate_output_schema(schema: &Value) -> Result<()> {
    jsonschema::meta::validate(schema)
        .map_err(|error| anyhow!("output schema is not a valid JSON Schema: {error}"))
}

pub(crate) fn validate_json_output(answer: &str, schema: &Value) -> Result<()> {
    let value: Value =
        serde_json::from_str(answer).map_err(|_| anyhow!("model output is not valid JSON"))?;
    validate_output_schema(schema)?;
    let validator = jsonschema::options()
        .with_pattern_options(jsonschema::PatternOptions::regex())
        .build(schema)
        .map_err(|error| anyhow!("output schema cannot be compiled: {error}"))?;
    validator.validate(&value).map_err(|error| {
        anyhow!(
            "model output does not match JSON Schema at {}: {error}",
            error.instance_path()
        )
    })
}

fn emit_json(value: Value) -> Result<()> {
    serde_json::to_writer(io::stdout().lock(), &value)?;
    println!();
    Ok(())
}

fn stream_record(event: &LiveEvent) -> Value {
    json!({
        "schema_version": "1",
        "sequence": event.id,
        "timestamp": event.timestamp,
        "kind": event.kind,
        "session_id": event.session_id,
        "turn_id": event.turn_id,
        "item_id": event.item_id,
        "payload": event.payload,
    })
}

fn turn_started_record(output_mode: OutputMode, session: &Id, turn: &Id) -> Option<Value> {
    matches!(output_mode, OutputMode::Jsonl | OutputMode::StreamJson).then(|| {
        json!({
            "schema_version": "1",
            "type": "turn.started",
            "session_id": session,
            "turn_id": turn,
        })
    })
}

fn lifecycle_record(
    output_mode: OutputMode,
    event: &LiveEvent,
    legacy_record: Value,
) -> Option<Value> {
    match output_mode {
        OutputMode::Text => None,
        OutputMode::Jsonl | OutputMode::StreamJson => Some(legacy_record),
        OutputMode::RawEventsJsonl => Some(stream_record(event)),
    }
}

fn tool_limit_resume_message(session: &Id) -> String {
    format!(
        "the turn reached its tool-call limit with unfinished work; run `s-code --resume={}` without `--print` to choose Resume unfinished turn or Stop",
        session.0
    )
}

fn question_required_message(
    kind: &str,
    payload: &Value,
    session: &Id,
    output_mode: OutputMode,
) -> Option<String> {
    if kind != "question.required" {
        return None;
    }
    if payload.get("source").and_then(Value::as_str) == Some("tool_call_limit") {
        return Some(tool_limit_resume_message(session));
    }
    // Other output modes keep waiting for the turn as before; raw-events
    // consumers run unattended and need the run to end instead.
    (output_mode == OutputMode::RawEventsJsonl)
        .then(|| "the turn requested interactive input; rerun without --print".into())
}

pub(crate) async fn print_turn(
    api: &Api,
    session: &Id,
    turn: &Id,
    output_mode: OutputMode,
) -> Result<String> {
    let path = format!(
        "/v1/events?organization_id={}&team_id={}&actor_id={}&after=0",
        encode(&api.scope.organization_id.0),
        encode(&api.scope.team_id.0),
        encode(&api.scope.actor_id.0),
    );
    let response = api.send(api.request(reqwest::Method::GET, &path)).await?;
    if !response.status().is_success() {
        return Err(anyhow!("event stream returned {}", response.status()));
    }
    let mut bytes = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut wrote = false;
    let mut answer = String::new();
    if let Some(record) = turn_started_record(output_mode, session, turn) {
        emit_json(record)?;
    }
    while let Some(chunk) = bytes.next().await {
        buffer.extend_from_slice(&chunk?);
        for event in drain_sse(&mut buffer).map_err(anyhow::Error::msg)? {
            if event.session_id.as_ref() != Some(session) || event.turn_id.as_ref() != Some(turn) {
                continue;
            }
            if let Some(message) =
                question_required_message(&event.kind, &event.payload, session, output_mode)
            {
                if let Some(record) = lifecycle_record(
                    output_mode,
                    &event,
                    json!({
                        "schema_version": "1",
                        "type": "question.required",
                        "session_id": session,
                        "turn_id": turn,
                        "item_id": event.item_id,
                        "payload": event.payload,
                    }),
                ) {
                    emit_json(record)?;
                }
                return Err(anyhow!(message));
            }
            match event.kind.as_str() {
                "model.delta" => {
                    let text = event.payload["text"].as_str().unwrap_or("");
                    answer.push_str(text);
                    match output_mode {
                        OutputMode::Text => {
                            print!("{text}");
                            io::stdout().flush()?;
                        }
                        OutputMode::Jsonl => emit_json(json!({
                            "schema_version": "1",
                            "type": "message.delta",
                            "session_id": session,
                            "turn_id": turn,
                            "item_id": event.item_id,
                            "delta": text,
                        }))?,
                        OutputMode::StreamJson | OutputMode::RawEventsJsonl => {
                            emit_json(stream_record(&event))?
                        }
                    }
                    wrote |= !text.is_empty();
                }
                "approval.required" => {
                    if let Some(record) = lifecycle_record(
                        output_mode,
                        &event,
                        json!({
                            "schema_version": "1",
                            "type": "approval.required",
                            "session_id": session,
                            "turn_id": turn,
                            "item_id": event.item_id,
                            "payload": event.payload,
                        }),
                    ) {
                        emit_json(record)?;
                    }
                    if let Some(id) = event.payload["approval_id"].as_str() {
                        match api.approval(id, false, ApprovalScope::Once).await {
                            Ok(_) => {}
                            Err(error)
                                if error.to_string().contains("approval is no longer pending") =>
                            {
                                // Automatic workspace decisions are resolved before their
                                // auditable request event is published. Keep consuming the Turn.
                                continue;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    return Err(anyhow!(
                        "the turn requested interactive approval; rerun without --print"
                    ));
                }
                "turn.completed" => {
                    let persisted = api
                        .messages(session)
                        .await?
                        .into_iter()
                        .rev()
                        .find(|message| message.turn_id == *turn && message.role == "assistant")
                        .map(|message| value_text(&message.content));
                    if answer.is_empty() {
                        answer = persisted.unwrap_or_default();
                    }
                    match output_mode {
                        OutputMode::Text if wrote => println!(),
                        OutputMode::Text => println!("{answer}"),
                        OutputMode::Jsonl => emit_json(json!({
                            "schema_version": "1",
                            "type": "turn.completed",
                            "session_id": session,
                            "turn_id": turn,
                            "message": answer,
                        }))?,
                        OutputMode::StreamJson | OutputMode::RawEventsJsonl => {
                            emit_json(stream_record(&event))?
                        }
                    }
                    return Ok(answer);
                }
                "turn.failed" => {
                    if let Some(record) = lifecycle_record(
                        output_mode,
                        &event,
                        json!({
                            "schema_version": "1",
                            "type": "turn.failed",
                            "session_id": session,
                            "turn_id": turn,
                            "error_code": event.payload["error_code"],
                        }),
                    ) {
                        emit_json(record)?;
                    }
                    return Err(anyhow!(
                        "turn failed: {}",
                        event.payload["error_code"]
                            .as_str()
                            .unwrap_or("unknown error")
                    ));
                }
                "turn.cancelled" => {
                    if let Some(record) = lifecycle_record(
                        output_mode,
                        &event,
                        json!({
                            "schema_version": "1",
                            "type": "turn.cancelled",
                            "session_id": session,
                            "turn_id": turn,
                        }),
                    ) {
                        emit_json(record)?;
                    }
                    return Err(anyhow!("turn was cancelled").context(CliExitStatus::Cancelled));
                }
                _ if matches!(
                    output_mode,
                    OutputMode::StreamJson | OutputMode::RawEventsJsonl
                ) =>
                {
                    emit_json(stream_record(&event))?
                }
                _ => {}
            }
        }
    }
    Err(anyhow!("event stream ended before the turn completed"))
}

#[cfg(test)]
mod tests {
    use super::{
        lifecycle_record, question_required_message, stream_record, tool_limit_resume_message,
        turn_started_record,
    };
    use crate::{args::OutputMode, state::LiveEvent};
    use chrono::Utc;
    use s_code_protocol::Id;
    use serde_json::json;

    #[test]
    fn stream_records_preserve_terminal_limit_details() {
        let event = LiveEvent {
            id: 7,
            timestamp: Utc::now(),
            kind: "turn.failed".into(),
            payload: json!({
                "error_code": "turn_elapsed_timeout",
                "reason": "turn elapsed-time limit reached",
            }),
            session_id: Some(Id("session".into())),
            turn_id: Some(Id("turn".into())),
            item_id: Some(Id("item".into())),
        };
        let record = stream_record(&event);
        assert_eq!(record["kind"], "turn.failed");
        assert_eq!(record["payload"], event.payload);
        assert!(record.get("type").is_none());
    }

    #[test]
    fn raw_events_are_additive_to_the_legacy_stream_json_contract() {
        let session = Id("session".into());
        let turn = Id("turn".into());
        let started = turn_started_record(OutputMode::StreamJson, &session, &turn).unwrap();
        assert_eq!(
            started,
            json!({
                "schema_version": "1",
                "type": "turn.started",
                "session_id": "session",
                "turn_id": "turn",
            })
        );
        assert!(turn_started_record(OutputMode::RawEventsJsonl, &session, &turn).is_none());

        let event = LiveEvent {
            id: 8,
            timestamp: Utc::now(),
            kind: "turn.failed".into(),
            payload: json!({"error_code":"turn_elapsed_timeout", "reason":"elapsed"}),
            session_id: Some(session),
            turn_id: Some(turn),
            item_id: None,
        };
        let legacy = json!({
            "schema_version": "1",
            "type": "turn.failed",
            "session_id": "session",
            "turn_id": "turn",
            "error_code": "turn_elapsed_timeout",
        });
        assert_eq!(
            lifecycle_record(OutputMode::StreamJson, &event, legacy.clone()),
            Some(legacy)
        );
        let raw = lifecycle_record(OutputMode::RawEventsJsonl, &event, json!({})).unwrap();
        assert_eq!(raw["kind"], "turn.failed");
        assert_eq!(raw["payload"], event.payload);
        assert!(raw.get("type").is_none());
    }

    #[test]
    fn questions_interrupt_only_where_unattended_output_needs_it() {
        let session = Id("ses_123".into());
        let tool_limit = json!({"source":"tool_call_limit"});
        let elicitation = json!({"source":"mcp_elicitation"});
        for mode in [
            OutputMode::Text,
            OutputMode::Jsonl,
            OutputMode::StreamJson,
            OutputMode::RawEventsJsonl,
        ] {
            assert_eq!(
                question_required_message("question.required", &tool_limit, &session, mode)
                    .as_deref(),
                Some(tool_limit_resume_message(&session).as_str())
            );
            assert!(
                question_required_message("turn.failed", &tool_limit, &session, mode).is_none()
            );
        }
        for mode in [OutputMode::Text, OutputMode::Jsonl, OutputMode::StreamJson] {
            assert!(
                question_required_message("question.required", &elicitation, &session, mode)
                    .is_none()
            );
        }
        assert_eq!(
            question_required_message(
                "question.required",
                &elicitation,
                &session,
                OutputMode::RawEventsJsonl,
            )
            .as_deref(),
            Some("the turn requested interactive input; rerun without --print")
        );
        assert!(
            tool_limit_resume_message(&Id("ses_123".into()))
                .contains("s-code --resume=ses_123` without `--print")
        );
    }
}
