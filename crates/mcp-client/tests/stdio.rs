use opencoding_mcp_client::{
    McpClient, McpElicitationHandler, McpElicitationRequest, McpElicitationResponse, McpError,
    McpProgressUpdate, McpRegistry, McpServerConfig,
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::mpsc;

#[tokio::test]
async fn stdio_client_rejects_an_oversized_response_without_a_newline() {
    let error = McpClient::connect(&McpServerConfig {
        id: "oversized".into(),
        program: env!("CARGO_BIN_EXE_mcp_fixture").into(),
        args: vec!["--oversized-no-newline".into()],
        environment_handles: BTreeMap::new(),
        timeout_ms: 2_000,
    })
    .await
    .err()
    .expect("oversized stdio response must fail");
    assert!(error.to_string().contains("exceeds 1 MiB"));
}

#[tokio::test]
async fn stdio_client_initializes_lists_and_calls_namespaced_ready_tools() {
    let client = McpClient::connect(&McpServerConfig {
        id: "fixture".into(),
        program: env!("CARGO_BIN_EXE_mcp_fixture").into(),
        args: vec![],
        environment_handles: BTreeMap::new(),
        timeout_ms: 2_000,
    })
    .await
    .unwrap();
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].input_schema["type"], "object");
    let output = client
        .call_tool("echo", json!({"text":"hello"}))
        .await
        .unwrap();
    assert_eq!(output["content"][0]["text"], "hello");
    let resources = client.list_resources(None).await.unwrap();
    assert_eq!(resources.entries[0].uri, "fixture://docs/readme");
    assert_eq!(resources.next_cursor.as_deref(), Some("page-2"));
    let templates = client.list_resource_templates(None).await.unwrap();
    assert_eq!(templates.entries[0].uri_template, "fixture://issues/{id}");
    let contents = client.read_resource("fixture://docs/readme").await.unwrap();
    assert_eq!(
        contents[0].text.as_deref(),
        Some("# Fixture resource\n\nSafe MCP content.")
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn stdio_tool_call_streams_bounded_ordered_progress() {
    let client = McpClient::connect(&McpServerConfig {
        id: "fixture".into(),
        program: env!("CARGO_BIN_EXE_mcp_fixture").into(),
        args: vec![],
        environment_handles: BTreeMap::new(),
        timeout_ms: 2_000,
    })
    .await
    .unwrap();
    let (progress_tx, mut progress_rx) = mpsc::channel::<McpProgressUpdate>(1);
    let call =
        client.call_tool_with_progress("echo", json!({"text":"with progress"}), Some(progress_tx));
    let collect = async {
        let mut updates = Vec::new();
        while let Some(update) = progress_rx.recv().await {
            updates.push(update);
        }
        updates
    };
    let (output, updates) = tokio::join!(call, collect);
    assert_eq!(output.unwrap()["content"][0]["text"], "with progress");
    assert_eq!(
        updates,
        vec![
            McpProgressUpdate {
                progress: 25.0,
                total: Some(100.0),
                message: Some("Preparing fixture".into()),
            },
            McpProgressUpdate {
                progress: 100.0,
                total: Some(100.0),
                message: Some("Fixture complete".into()),
            },
        ]
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn registry_namespaces_tools_and_routes_calls() {
    let registry = McpRegistry::connect(&[McpServerConfig {
        id: "team_docs".into(),
        program: env!("CARGO_BIN_EXE_mcp_fixture").into(),
        args: vec![],
        environment_handles: BTreeMap::new(),
        timeout_ms: 2_000,
    }])
    .await
    .unwrap();
    assert_eq!(registry.definitions()[0].name, "mcp.team_docs.echo");
    assert!(registry.contains("mcp.team_docs.echo"));
    assert_eq!(registry.server_ids(), vec!["team_docs"]);
    assert_eq!(
        registry
            .call("mcp.team_docs.echo", json!({"text":"routed"}))
            .await
            .unwrap()["content"][0]["text"],
        "routed"
    );
    assert_eq!(
        registry
            .list_resources("team_docs", None)
            .await
            .unwrap()
            .entries[0]
            .name,
        "README"
    );
    assert_eq!(
        registry
            .read_resource("team_docs", "fixture://docs/readme")
            .await
            .unwrap()[0]
            .mime_type
            .as_deref(),
        Some("text/markdown")
    );
}

struct SafeElicitation;

#[async_trait::async_trait]
impl McpElicitationHandler for SafeElicitation {
    async fn elicit(
        &self,
        request: McpElicitationRequest,
    ) -> Result<McpElicitationResponse, McpError> {
        assert_eq!(request.server_id, "team_docs");
        assert_eq!(request.request_id, "fixture-elicitation");
        assert_eq!(
            request.requested_schema["properties"]["display_name"]["type"],
            "string"
        );
        Ok(McpElicitationResponse::Accept {
            content: json!({"display_name": "Reviewed name"}),
        })
    }
}

#[tokio::test]
async fn stdio_elicitation_is_explicitly_handled_inside_the_originating_tool_call() {
    let registry = McpRegistry::connect(&[McpServerConfig {
        id: "team_docs".into(),
        program: env!("CARGO_BIN_EXE_mcp_fixture").into(),
        args: vec![],
        environment_handles: BTreeMap::new(),
        timeout_ms: 2_000,
    }])
    .await
    .unwrap();
    let output = registry
        .call_with_progress_and_elicitation(
            "mcp.team_docs.elicit",
            json!({}),
            None,
            Some(Arc::new(SafeElicitation)),
        )
        .await
        .unwrap();
    assert_eq!(output["content"][0]["text"], "Reviewed name");
}

#[test]
fn configuration_rejects_shell_resolution_and_raw_environment_values() {
    let relative = McpServerConfig {
        id: "fixture".into(),
        program: "python3".into(),
        args: vec![],
        environment_handles: BTreeMap::new(),
        timeout_ms: 1_000,
    };
    assert!(relative.validate().is_err());
    let mut unsafe_environment = BTreeMap::new();
    unsafe_environment.insert("TOKEN=value".into(), "RAW SECRET".into());
    let invalid = McpServerConfig {
        program: "/bin/echo".into(),
        environment_handles: unsafe_environment,
        ..relative
    };
    assert!(invalid.validate().is_err());
}
