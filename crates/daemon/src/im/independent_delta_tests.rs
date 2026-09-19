// Independent regressions for the human-review follow-up. Included by tests.rs.

async fn delta_approval(content: &str) -> (Fixture, Approval, s_code_protocol::ToolCall) {
    let f = Fixture::new(vec![]).await;
    let turn = f
        .state
        .store
        .create_turn(&f.scope, &f.session.id)
        .await
        .unwrap();
    let call = f
        .state
        .store
        .create_tool_call(
            s_code_protocol::ToolRequest {
                parent_tool_call_id: None,
                id: Id::new("tool"),
                scope: f.scope.clone(),
                session_id: f.session.id.clone(),
                turn_id: turn.id,
                tool: "apply_patch".into(),
                arguments: json!({"path":"created.txt","expected_sha256":null,"content":content}),
                created_at: Utc::now(),
            },
            s_code_protocol::PolicyResult {
                decision: s_code_protocol::PolicyDecision::Ask,
                policy_id: "test".into(),
                policy_version: "1".into(),
                reason: "Review before execution".into(),
                requires_approval: true,
            },
            s_code_storage::ToolPolicyMetadata::default(),
            s_code_protocol::ToolCallStatus::AwaitingApproval,
        )
        .await
        .unwrap();
    let approval = f.state.store.create_approval(&call).await.unwrap();
    (f, approval, call)
}

#[tokio::test]
async fn independent_printable_unicode_and_literal_escape_sequences_remain_approvable() {
    // Literal backslash-u is visible text, not an actual formatting control.
    for text in [
        "你好，世界",
        "café",
        "שלום",
        "مرحبا",
        "🦀",
        r"literal \u202e",
        "line one\nline two\tend",
    ] {
        let (_f, approval, call) = delta_approval(text).await;
        let (preview, allowed) = phone_approval_preview(&approval, &call).unwrap();
        assert!(
            allowed,
            "ordinary visible text should remain supported: {text:?}"
        );
        assert!(!preview.is_empty());
    }
}

#[tokio::test]
async fn independent_invisible_fillers_and_unicode_formats_never_get_phone_approval() {
    for codepoint in [
        0x115f, 0x1160, 0x3164, 0xffa0, 0x2800, 0x17b4, 0x17b5, 0x00ad, 0x034f, 0x180e, 0x200b,
        0x200c, 0x200d, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2060, 0x2066, 0x2067, 0x2068,
        0x2069, 0xfeff, 0xe0061,
    ] {
        let hidden = char::from_u32(codepoint).unwrap();
        let content = format!("safe{hidden}looking");
        let (_f, approval, call) = delta_approval(&content).await;
        assert!(
            !phone_approval_preview(&approval, &call).unwrap().1,
            "U+{codepoint:04X} must require local review"
        );
        let display = s_code_connector_sdk::im::ImUserDisplay {
            first_name: Some(content),
            username: None,
        }
        .sanitized();
        let shown = display.first_name.unwrap();
        assert!(
            !shown.contains(hidden),
            "U+{codepoint:04X} must not remain invisible in a pairing identity"
        );
        assert!(shown.contains("safe") && shown.contains("looking"));
    }
}

#[tokio::test]
async fn independent_old_pairing_state_loads_without_display_metadata() {
    let mut f = Fixture::new(vec![]).await;
    let _ = f.manage(ManageAction::Revoke {}).await.unwrap();
    let mut value = serde_json::to_value(&f.loaded.data).unwrap();
    value["pairing"] = json!({
        "code_hash":digest("legacy-code"), "expires":Utc::now().timestamp()+600,
        "pending_user":42, "pending_chat":42,
    });
    let stored = f
        .state
        .store
        .put_im_state(&f.scope, CHANNEL, f.loaded.revision, &value)
        .await
        .unwrap();
    f.loaded = Loaded::load(&f.state, &f.scope).await.unwrap();
    assert_eq!(f.loaded.revision, Some(stored.revision));
    let status = public_status(&f.loaded.data);
    assert_eq!(status["pending_user"], 42);
    assert_eq!(status["pending_identity"]["user_id"], 42);
    assert!(status["pending_identity"]["first_name"].is_null());
    assert!(status["pending_identity"]["username"].is_null());
    let _ = f
        .manage(ManageAction::Approve { user_id: 42 })
        .await
        .unwrap();
    assert_eq!(f.loaded.data.binding.as_ref().unwrap().user_id, 42);
}

#[tokio::test]
async fn independent_repeat_claim_updates_labels_without_changing_numeric_identity() {
    let mut f = Fixture::new(vec![]).await;
    let _ = f.manage(ManageAction::Revoke {}).await.unwrap();
    let response = f.manage(ManageAction::Pair {}).await.unwrap().0;
    let code = response["pairing_link"]
        .as_str()
        .unwrap()
        .split("?start=")
        .nth(1)
        .unwrap();
    for (update_id, label) in [(1, "Original"), (2, "Renamed")] {
        let mut update = message(update_id, 42, 42, &format!("/start {code}"));
        update.user_display.first_name = Some(label.into());
        update.user_display.username = Some("alice".into());
        process_claimed_update(&f.state, &f.scope, &mut f.loaded, &f.channel, update)
            .await
            .unwrap();
    }
    let status = public_status(&f.loaded.data);
    assert_eq!(status["pending_identity"]["user_id"], 42);
    assert_eq!(status["pending_identity"]["first_name"], "Renamed");
    let sent = f.channel.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].text.contains("Telegram user ID: 42\n"));
    assert!(sent[1].text.contains("Name: Renamed"));
    assert!(sent[1].text.contains("display-only"));
}
