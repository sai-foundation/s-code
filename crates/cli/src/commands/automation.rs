use crate::{
    CliExitStatus,
    api::{Api, encode},
    args::OutputMode,
    commands::interactive::{drain_sse, value_text},
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
    if output_mode != OutputMode::Text {
        emit_json(json!({
            "schema_version": "1",
            "type": "turn.started",
            "session_id": session,
            "turn_id": turn,
        }))?;
    }
    while let Some(chunk) = bytes.next().await {
        buffer.extend_from_slice(&chunk?);
        for event in drain_sse(&mut buffer).map_err(anyhow::Error::msg)? {
            if event.session_id.as_ref() != Some(session) || event.turn_id.as_ref() != Some(turn) {
                continue;
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
                        OutputMode::StreamJson => emit_json(json!({
                            "schema_version": "1",
                            "sequence": event.id,
                            "timestamp": event.timestamp,
                            "kind": event.kind,
                            "session_id": event.session_id,
                            "turn_id": event.turn_id,
                            "item_id": event.item_id,
                            "payload": event.payload,
                        }))?,
                    }
                    wrote |= !text.is_empty();
                }
                "approval.required" => {
                    if output_mode != OutputMode::Text {
                        emit_json(json!({
                            "schema_version": "1",
                            "type": "approval.required",
                            "session_id": session,
                            "turn_id": turn,
                            "item_id": event.item_id,
                            "payload": event.payload,
                        }))?;
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
                        OutputMode::StreamJson => emit_json(json!({
                            "schema_version": "1",
                            "sequence": event.id,
                            "timestamp": event.timestamp,
                            "kind": event.kind,
                            "session_id": event.session_id,
                            "turn_id": event.turn_id,
                            "item_id": event.item_id,
                            "payload": event.payload,
                        }))?,
                    }
                    return Ok(answer);
                }
                "turn.failed" => {
                    if output_mode != OutputMode::Text {
                        emit_json(json!({
                            "schema_version": "1",
                            "type": "turn.failed",
                            "session_id": session,
                            "turn_id": turn,
                            "error_code": event.payload["error_code"],
                        }))?;
                    }
                    return Err(anyhow!(
                        "turn failed: {}",
                        event.payload["error_code"]
                            .as_str()
                            .unwrap_or("unknown error")
                    ));
                }
                "turn.cancelled" => {
                    if output_mode != OutputMode::Text {
                        emit_json(json!({
                            "schema_version": "1",
                            "type": "turn.cancelled",
                            "session_id": session,
                            "turn_id": turn,
                        }))?;
                    }
                    return Err(anyhow!("turn was cancelled").context(CliExitStatus::Cancelled));
                }
                _ if output_mode == OutputMode::StreamJson => emit_json(json!({
                    "schema_version": "1",
                    "sequence": event.id,
                    "timestamp": event.timestamp,
                    "kind": event.kind,
                    "session_id": event.session_id,
                    "turn_id": event.turn_id,
                    "item_id": event.item_id,
                    "payload": event.payload,
                }))?,
                _ => {}
            }
        }
    }
    Err(anyhow!("event stream ended before the turn completed"))
}
