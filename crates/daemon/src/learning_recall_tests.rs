use super::*;
use s_code_protocol::{PolicyDecision, PolicyResult, ToolCall, ToolRequest};

#[test]
fn independent_location_and_visibility_cases() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/selective-recall-independent-cases.json"
    ))
    .unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 27);
    for case in cases {
        let id = case["id"].as_str().unwrap();
        let source: ProjectSourceObservation =
            serde_json::from_value(case["observation"].clone()).unwrap();
        if case["operation"] == "already_visible" {
            let messages: Vec<ModelMessage> =
                serde_json::from_value(case["messages"].clone()).unwrap();
            assert_eq!(
                already_visible(&source, &messages),
                case["expected_suppressed"].as_bool().unwrap(),
                "{id}"
            );
        } else {
            let calls = case["lookups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    let mut call = lookup(
                        row["tool"].as_str().unwrap(),
                        row["arguments"].clone(),
                        row["result"].clone(),
                    );
                    call.status = serde_json::from_value(row["status"].clone()).unwrap();
                    call.created_at = serde_json::from_value(row["created_at"].clone()).unwrap();
                    call.updated_at = serde_json::from_value(row["updated_at"].clone()).unwrap();
                    call.request.created_at = call.created_at;
                    call.error = serde_json::from_value(row["error"].clone()).unwrap();
                    call
                })
                .collect::<Vec<_>>();
            let selected = focus(&calls);
            let expected = case["expected_score"].as_u64().unwrap_or_else(|| {
                // The independent design left citation policy open. This
                // candidate deliberately does not parse path:line suffixes.
                assert_eq!(id, "P07");
                0
            });
            assert_eq!(
                score(case["prompt"].as_str().unwrap(), &source, selected.as_ref()),
                expected as usize,
                "{id}"
            );
        }
    }
}

fn observation() -> ProjectSourceObservation {
    ProjectSourceObservation {
        change: Some(SourceChangeEvidence {
            previous_sha256: None,
        }),
        path: "src/task_queue.py".into(),
        sha256: "a".repeat(64),
        start_line: 10,
        end_line: 11,
        fragments: vec![SourceFragment {
            start_line: 10,
            text: "def expire_pending():\n    return 3\n".into(),
        }],
        truncated: false,
    }
}

fn lookup(name: &str, arguments: serde_json::Value, result: serde_json::Value) -> ToolCall {
    let now = Utc::now();
    ToolCall {
        request: ToolRequest {
            id: Id::new("tool"),
            scope: Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("actor".into()),
                goal_id: None,
                task_id: None,
            },
            session_id: Id("session".into()),
            turn_id: Id("turn".into()),
            tool: name.into(),
            arguments,
            created_at: now,
        },
        policy: PolicyResult {
            decision: PolicyDecision::Allow,
            policy_id: "fixture".into(),
            policy_version: "1".into(),
            reason: "fixture".into(),
            requires_approval: false,
        },
        simulated_policy: None,
        central_policy_applied: false,
        team_configuration_sequence: None,
        status: ToolCallStatus::Completed,
        result: Some(result),
        error: None,
        created_at: now,
        updated_at: now,
    }
}

fn search(query: &str, output: &str) -> ToolCall {
    lookup(
        "search_text",
        serde_json::json!({"query":query}),
        serde_json::json!({"stdout":output,"stderr":"","exit_code":0,"truncated":false}),
    )
}

#[test]
fn explicit_paths_preserve_names_and_boundaries() {
    let source = observation();
    for prompt in [
        "Change src/task_queue.py",
        "Review `./src/task_queue.py`.",
        "检查 (src/task_queue.py)",
        "src/task_queue.py: verify it",
    ] {
        assert_eq!(score(prompt, &source, None), 2, "{prompt}");
    }
    for prompt in [
        "Improve the pending queue",
        "Update expire_pending",
        "task_queue.py",
        "src/task_queue.py.bak",
        "vendor/src/task_queue.py",
        "../src/task_queue.py",
        "/src/task_queue.py",
        "https://host/src/task_queue.py",
        "SRC/task_queue.py",
        "src/task_queue.py/extra",
        "prefix_src/task_queue.py",
    ] {
        assert_eq!(score(prompt, &source, None), 0, "{prompt}");
    }
    let mut spaced = source;
    spaced.path = "src/queue helper.py".into();
    assert_eq!(score("Edit `src/queue helper.py`", &spaced, None), 2);
    spaced.change = None;
    assert_eq!(score("Edit `src/queue helper.py`", &spaced, None), 0);
}

#[test]
fn search_requires_an_unambiguous_literal_location_in_retained_lines() {
    let source = observation();
    let call = search(
        "expire_pending",
        "src/task_queue.py:10:def expire_pending():\n",
    );
    let selected = focus(&[call]).unwrap();
    assert_eq!(score("Improve cancellation", &source, Some(&selected)), 1);
    for (query, output) in [
        ("expire.*", "src/task_queue.py:10:def expire_pending():\n"),
        (
            "expire_pending",
            "src/task_queue.py:10:def expire_pending_later():\n",
        ),
        (
            "expire_pending",
            "src/task_queue.py:10:def expire_pending():\nother.py:1:expire_pending()\n",
        ),
        (
            "expire_pending",
            "../src/task_queue.py:10:expire_pending()\n",
        ),
        ("expire_pending", "src/task_queue.py:0:expire_pending()\n"),
    ] {
        assert!(
            focus(&[search(query, output)]).is_none(),
            "{query}: {output}"
        );
    }
    let outside = focus(&[search(
        "expire_pending",
        "src/task_queue.py:90:expire_pending()\n",
    )])
    .unwrap();
    assert_eq!(score("Improve cancellation", &source, Some(&outside)), 0);
    let mut truncated = search("expire_pending", "src/task_queue.py:10:expire_pending()\n");
    truncated.result.as_mut().unwrap()["truncated"] = true.into();
    assert!(focus(&[truncated]).is_none());
}

#[test]
fn latest_failed_or_ambiguous_lookup_does_not_reuse_old_focus() {
    let first = search("expire_pending", "src/task_queue.py:10:expire_pending()\n");
    let mut second = first.clone();
    second.created_at += chrono::Duration::seconds(1);
    second.updated_at = second.created_at;
    second.status = ToolCallStatus::Failed;
    assert!(focus(&[first.clone(), second]).is_none());
    assert!(focus(&[first.clone(), first]).is_none());
}

#[test]
fn old_search_text_or_a_later_mutation_cannot_anchor_current_code() {
    let source = observation();
    let stale = focus(&[search("old_name", "src/task_queue.py:10:def old_name():\n")]).unwrap();
    assert_eq!(score("Improve cancellation", &source, Some(&stale)), 0);
    let first = search(
        "expire_pending",
        "src/task_queue.py:10:def expire_pending():\n",
    );
    let mut mutation = lookup("apply_patch", serde_json::json!({}), serde_json::json!({}));
    mutation.created_at = first.created_at + chrono::Duration::seconds(1);
    mutation.updated_at = mutation.created_at;
    assert!(focus(&[first.clone(), mutation.clone()]).is_none());
    mutation.created_at = first.created_at - chrono::Duration::seconds(1);
    mutation.updated_at = mutation.created_at;
    mutation.status = ToolCallStatus::Running;
    assert!(focus(&[first, mutation]).is_none());
}

#[test]
fn read_focus_requires_matching_path_and_current_hash() {
    let source = observation();
    let call = lookup(
        "read_file",
        serde_json::json!({"path":"./src/task_queue.py"}),
        serde_json::json!({"path":"src/task_queue.py","sha256":"a".repeat(64),"content":"def expire_pending():\n","start_line":10,"end_line":10}),
    );
    let selected = focus(std::slice::from_ref(&call)).unwrap();
    assert_eq!(score("Improve cancellation", &source, Some(&selected)), 1);
    let mut changed = source.clone();
    changed.sha256 = "b".repeat(64);
    assert_eq!(score("Improve cancellation", &changed, Some(&selected)), 0);
    let mut wrong = call;
    wrong.result.as_mut().unwrap()["path"] = "other.py".into();
    assert!(focus(&[wrong]).is_none());
}

#[test]
fn visible_fragments_need_exact_file_hash_position_and_literal_content() {
    let source = observation();
    let message = ModelMessage {
        role: "tool".into(),
        content: serde_json::json!({
            "name":"read_file", "tool_call_id":"call_1", "result":{
                "path":"src/task_queue.py", "sha256":"a".repeat(64), "start_line":9,
                "content":"# heading\ndef expire_pending():\n    return 3\n"}
        }),
    };
    assert!(already_visible(&source, std::slice::from_ref(&message)));
    for mutation in ["role", "hash", "line", "content", "path"] {
        let mut bad = message.clone();
        match mutation {
            "role" => bad.role = "user".into(),
            "hash" => bad.content["result"]["sha256"] = "b".repeat(64).into(),
            "line" => bad.content["result"]["start_line"] = 8.into(),
            "content" => {
                bad.content["result"]["content"] = "# heading\ndef expire_pending():\n".into()
            }
            "path" => bad.content["result"]["path"] = "other.py".into(),
            _ => unreachable!(),
        }
        assert!(!already_visible(&source, &[bad]), "{mutation}");
    }
}
