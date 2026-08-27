use futures_util::StreamExt;
use reqwest::{
    Client as HttpClient, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    path::Path,
    process::Stdio,
    str::FromStr,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, mpsc},
};

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_PROGRESS_MESSAGE_BYTES: usize = 1024;
const MAX_TOOLS: usize = 1_000;
const MAX_RESOURCES: usize = 1_000;
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Error)]
pub enum McpError {
    #[error("invalid MCP configuration: {0}")]
    Configuration(String),
    #[error("MCP process I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("MCP server requires authentication")]
    AuthenticationRequired,
    #[error("MCP response timed out")]
    Timeout,
    #[error("MCP response is invalid: {0}")]
    Protocol(String),
    #[error("MCP server returned error: {0}")]
    Server(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub environment_handles: BTreeMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHttpServerConfig {
    pub id: String,
    pub endpoint: String,
    #[serde(default)]
    pub header_handles: BTreeMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    30_000
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceTemplate {
    pub uri_template: String,
    pub name: String,
    pub description: String,
    pub mime_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourcePage<T> {
    pub entries: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob_base64: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpProgressUpdate {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpElicitationRequest {
    pub request_id: Value,
    pub server_id: String,
    pub message: String,
    pub requested_schema: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum McpElicitationResponse {
    Accept { content: Value },
    Decline,
    Cancel,
}

#[async_trait::async_trait]
pub trait McpElicitationHandler: Send + Sync {
    async fn elicit(
        &self,
        request: McpElicitationRequest,
    ) -> Result<McpElicitationResponse, McpError>;
}

#[async_trait::async_trait]
pub trait McpHttpAuthorizationProvider: Send + Sync {
    async fn bearer_token(&self, server_id: &str) -> Result<Option<String>, McpError>;
}

struct Connection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct McpHttpClient {
    server_id: String,
    endpoint: url::Url,
    protocol_version: String,
    session_id: Option<String>,
    headers: HeaderMap,
    authorization: Option<Arc<dyn McpHttpAuthorizationProvider>>,
    client: HttpClient,
    next_id: AtomicU64,
    next_progress_token: AtomicU64,
}

pub struct McpClient {
    server_id: String,
    timeout: Duration,
    next_id: AtomicU64,
    next_progress_token: AtomicU64,
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RegisteredMcpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Clone)]
struct RegisteredTool {
    client: ConnectedMcpClient,
    upstream_name: String,
    definition: RegisteredMcpTool,
}

#[derive(Clone, Default)]
pub struct McpRegistry {
    tools: Arc<RwLock<BTreeMap<String, RegisteredTool>>>,
    clients: Arc<RwLock<BTreeMap<String, ConnectedMcpClient>>>,
}

#[derive(Clone)]
enum ConnectedMcpClient {
    Stdio(Arc<McpClient>),
    Http(Arc<McpHttpClient>),
}

impl McpServerConfig {
    pub fn validate(&self) -> Result<(), McpError> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
            || !Path::new(&self.program).is_absolute()
            || self.args.len() > 128
            || self.args.iter().any(|value| value.len() > 4096)
            || self.environment_handles.len() > 64
            || self.environment_handles.iter().any(|(name, handle)| {
                !valid_environment_name(name)
                    || !valid_environment_name(handle)
                    || name.len() > 128
                    || handle.len() > 128
            })
            || !(100..=120_000).contains(&self.timeout_ms)
        {
            return Err(McpError::Configuration(
                "id, absolute program, bounded argv/environment handles and 100..120000ms timeout are required".into(),
            ));
        }
        Ok(())
    }
}

impl McpHttpServerConfig {
    pub fn validate(&self) -> Result<(), McpError> {
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'-' | b'_'))
            || self.header_handles.len() > 64
            || self.header_handles.iter().any(|(name, handle)| {
                HeaderName::from_str(name).is_err()
                    || !valid_environment_name(handle)
                    || name.len() > 128
                    || handle.len() > 128
            })
            || !(100..=120_000).contains(&self.timeout_ms)
        {
            return Err(McpError::Configuration(
                "id, bounded HTTP header handles and 100..120000ms timeout are required".into(),
            ));
        }
        let endpoint = url::Url::parse(&self.endpoint)
            .map_err(|_| McpError::Configuration("MCP HTTP endpoint is invalid".into()))?;
        if endpoint.username() != ""
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.host_str().is_none()
            || !matches!(endpoint.scheme(), "http" | "https")
        {
            return Err(McpError::Configuration(
                "MCP HTTP endpoint requires an http(s) URL without credentials, query, or fragment"
                    .into(),
            ));
        }
        if endpoint.scheme() == "http" && !is_loopback_host(endpoint.host_str().unwrap_or_default())
        {
            return Err(McpError::Configuration(
                "unencrypted MCP HTTP is allowed only on a loopback address".into(),
            ));
        }
        Ok(())
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

impl McpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        config.validate()?;
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (name, handle) in &config.environment_handles {
            let value = std::env::var(handle).map_err(|_| {
                McpError::Configuration(format!("environment handle {handle} is unavailable"))
            })?;
            command.env(name, value);
        }
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Protocol("stdin pipe is unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Protocol("stdout pipe is unavailable".into()))?;
        let client = Self {
            server_id: config.id.clone(),
            timeout: Duration::from_millis(config.timeout_ms),
            next_id: AtomicU64::new(1),
            next_progress_token: AtomicU64::new(1),
            connection: Mutex::new(Connection {
                child,
                stdin,
                stdout: BufReader::new(stdout),
            }),
        };
        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"elicitation": {}},
                    "clientInfo": {"name": "opencoding", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        let raw = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| McpError::Protocol("tools/list result has no tools array".into()))?;
        if raw.len() > MAX_TOOLS {
            return Err(McpError::Protocol("server returned too many tools".into()));
        }
        let mut names = BTreeSet::new();
        raw.iter()
            .map(|value| {
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| McpError::Protocol("tool name is missing".into()))?;
                if name.is_empty()
                    || name.len() > 128
                    || !names.insert(name.to_owned())
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    })
                {
                    return Err(McpError::Protocol(
                        "tool names must be unique, bounded safe identifiers".into(),
                    ));
                }
                let input_schema = value
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"}));
                if !input_schema.is_object() {
                    return Err(McpError::Protocol(
                        "tool inputSchema must be an object".into(),
                    ));
                }
                let output_schema = value
                    .get("outputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"}));
                if !output_schema.is_object() {
                    return Err(McpError::Protocol(
                        "tool outputSchema must be an object".into(),
                    ));
                }
                Ok(McpTool {
                    name: name.into(),
                    description: value
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .chars()
                        .take(4096)
                        .collect(),
                    input_schema,
                    output_schema,
                })
            })
            .collect()
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.call_tool_with_progress(name, arguments, None).await
    }

    pub async fn call_tool_with_progress(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
    ) -> Result<Value, McpError> {
        self.call_tool_with_progress_and_elicitation(name, arguments, progress, None)
            .await
    }

    pub async fn call_tool_with_progress_and_elicitation(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        if name.is_empty() || name.len() > 128 || !arguments.is_object() {
            return Err(McpError::Protocol(
                "tool call requires a bounded name and object arguments".into(),
            ));
        }
        let progress_token = format!(
            "opencoding-{}-{}",
            self.server_id,
            self.next_progress_token.fetch_add(1, Ordering::Relaxed)
        );
        self.request_with_progress(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
                "_meta": {"progressToken": progress_token},
            }),
            Some(&progress_token),
            progress,
            elicitation,
        )
        .await
    }

    pub async fn list_resources(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResource>, McpError> {
        let result = self
            .request("resources/list", cursor_parameters(cursor)?)
            .await?;
        parse_resource_page(&result, "resources", |value| {
            Ok(McpResource {
                uri: resource_string(value, "uri", 4096)?,
                name: resource_string(value, "name", 512)?,
                description: optional_resource_string(value, "description", 4096)?
                    .unwrap_or_default(),
                mime_type: optional_resource_string(value, "mimeType", 256)?,
            })
        })
    }

    pub async fn list_resource_templates(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResourceTemplate>, McpError> {
        let result = self
            .request("resources/templates/list", cursor_parameters(cursor)?)
            .await?;
        parse_resource_page(&result, "resourceTemplates", |value| {
            Ok(McpResourceTemplate {
                uri_template: resource_string(value, "uriTemplate", 4096)?,
                name: resource_string(value, "name", 512)?,
                description: optional_resource_string(value, "description", 4096)?
                    .unwrap_or_default(),
                mime_type: optional_resource_string(value, "mimeType", 256)?,
            })
        })
    }

    pub async fn read_resource(&self, uri: &str) -> Result<Vec<McpResourceContent>, McpError> {
        validate_resource_value(uri, "resource URI", 4096)?;
        let result = self.request("resources/read", json!({"uri": uri})).await?;
        let contents = result
            .get("contents")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                McpError::Protocol("resources/read result has no contents array".into())
            })?;
        if contents.len() > MAX_RESOURCES {
            return Err(McpError::Protocol(
                "server returned too many resource contents".into(),
            ));
        }
        contents
            .iter()
            .map(|value| {
                let uri = resource_string(value, "uri", 4096)?;
                let mime_type = optional_resource_string(value, "mimeType", 256)?;
                let text = optional_resource_string(value, "text", MAX_MESSAGE_BYTES)?;
                let blob_base64 = optional_resource_string(value, "blob", MAX_MESSAGE_BYTES)?;
                if text.is_some() == blob_base64.is_some() {
                    return Err(McpError::Protocol(
                        "resource content must contain exactly one of text or blob".into(),
                    ));
                }
                Ok(McpResourceContent {
                    uri,
                    mime_type,
                    text,
                    blob_base64,
                })
            })
            .collect()
    }

    pub async fn shutdown(&self) -> Result<(), McpError> {
        let mut connection = self.connection.lock().await;
        connection.child.kill().await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let message = json!({"jsonrpc":"2.0","method":method,"params":params});
        let encoded =
            serde_json::to_vec(&message).map_err(|error| McpError::Protocol(error.to_string()))?;
        if encoded.len() > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol("request exceeds 1 MiB".into()));
        }
        let mut connection = self.connection.lock().await;
        connection.stdin.write_all(&encoded).await?;
        connection.stdin.write_all(b"\n").await?;
        connection.stdin.flush().await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with_progress(method, params, None, None, None)
            .await
    }

    async fn request_with_progress(
        &self,
        method: &str,
        params: Value,
        progress_token: Option<&str>,
        progress_sender: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let encoded =
            serde_json::to_vec(&message).map_err(|error| McpError::Protocol(error.to_string()))?;
        if encoded.len() > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol("request exceeds 1 MiB".into()));
        }
        let timeout = self.timeout;
        let mut connection = self.connection.lock().await;
        connection.stdin.write_all(&encoded).await?;
        connection.stdin.write_all(b"\n").await?;
        connection.stdin.flush().await?;
        tokio::time::timeout(timeout, async {
            loop {
                let mut line = String::new();
                let bytes = connection.stdout.read_line(&mut line).await?;
                if bytes == 0 {
                    return Err(McpError::Protocol("server closed stdout".into()));
                }
                if bytes > MAX_MESSAGE_BYTES {
                    return Err(McpError::Protocol("response exceeds 1 MiB".into()));
                }
                let response: Value = serde_json::from_str(&line)
                    .map_err(|error| McpError::Protocol(error.to_string()))?;
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    if response.get("method").and_then(Value::as_str) == Some("elicitation/create")
                    {
                        let reply =
                            elicitation_reply(&self.server_id, &response, elicitation.as_ref())
                                .await?;
                        let encoded = serde_json::to_vec(&reply)
                            .map_err(|error| McpError::Protocol(error.to_string()))?;
                        connection.stdin.write_all(&encoded).await?;
                        connection.stdin.write_all(b"\n").await?;
                        connection.stdin.flush().await?;
                        continue;
                    } else if response.get("id").is_none() {
                        if response.get("method").and_then(Value::as_str)
                            == Some("notifications/progress")
                            && let (Some(expected), Some(sender)) =
                                (progress_token, progress_sender.as_ref())
                            && let Some(update) = parse_progress_notification(&response, expected)?
                        {
                            // Keep draining the request if its observer disappears. Dropping the
                            // in-flight stdio response would poison later request IDs and falsely
                            // report an already-running tool as failed.
                            let _ = sender.send(update).await;
                        }
                        continue;
                    }
                    return Err(McpError::Protocol("response id mismatch".into()));
                }
                if let Some(error) = response.get("error") {
                    return Err(McpError::Server(error.to_string()));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| McpError::Protocol("response result is missing".into()));
            }
        })
        .await
        .map_err(|_| McpError::Timeout)?
    }
}

impl McpHttpClient {
    pub async fn connect(config: &McpHttpServerConfig) -> Result<Self, McpError> {
        Self::connect_with_authorization(config, None).await
    }

    pub async fn connect_with_authorization(
        config: &McpHttpServerConfig,
        authorization: Option<Arc<dyn McpHttpAuthorizationProvider>>,
    ) -> Result<Self, McpError> {
        config.validate()?;
        let endpoint = url::Url::parse(&config.endpoint)
            .map_err(|_| McpError::Configuration("MCP HTTP endpoint is invalid".into()))?;
        let mut headers = HeaderMap::new();
        for (name, handle) in &config.header_handles {
            let value = std::env::var(handle).map_err(|_| {
                McpError::Configuration(format!("environment handle {handle} is unavailable"))
            })?;
            let name = HeaderName::from_str(name)
                .map_err(|_| McpError::Configuration("MCP HTTP header name is invalid".into()))?;
            let value = HeaderValue::from_str(&value).map_err(|_| {
                McpError::Configuration(format!(
                    "environment handle {handle} is not a valid HTTP header value"
                ))
            })?;
            headers.insert(name, value);
        }
        let client = HttpClient::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()?;
        let initializing = Self {
            server_id: config.id.clone(),
            endpoint,
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            session_id: None,
            headers,
            authorization,
            client,
            next_id: AtomicU64::new(2),
            next_progress_token: AtomicU64::new(1),
        };
        let (response_headers, messages) = initializing
            .send_message(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {"elicitation": {}},
                        "clientInfo": {
                            "name": "opencoding",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                }),
                false,
            )
            .await?;
        let result = response_result(messages, 1)?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::Protocol("initialize result has no protocolVersion".into()))?;
        if !matches!(protocol_version, "2025-03-26" | MCP_PROTOCOL_VERSION) {
            return Err(McpError::Protocol(format!(
                "unsupported MCP protocol version {protocol_version}"
            )));
        }
        let session_id = response_headers
            .get("mcp-session-id")
            .map(|value| {
                let value = value.to_str().map_err(|_| {
                    McpError::Protocol("Mcp-Session-Id is not visible ASCII".into())
                })?;
                if value.is_empty()
                    || value.len() > 1024
                    || !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
                {
                    return Err(McpError::Protocol(
                        "Mcp-Session-Id must be bounded visible ASCII".into(),
                    ));
                }
                Ok(value.to_owned())
            })
            .transpose()?;
        let initialized = Self {
            protocol_version: protocol_version.into(),
            session_id,
            ..initializing
        };
        initialized
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(initialized)
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let result = self.request("tools/list", json!({})).await?;
        parse_tools(&result)
    }

    async fn call_tool_with_progress(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        if name.is_empty() || name.len() > 128 || !arguments.is_object() {
            return Err(McpError::Protocol(
                "tool call requires a bounded name and object arguments".into(),
            ));
        }
        let progress_token = format!(
            "opencoding-{}-{}",
            self.server_id,
            self.next_progress_token.fetch_add(1, Ordering::Relaxed)
        );
        self.request_with_progress(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments,
                "_meta": {"progressToken": progress_token},
            }),
            Some(&progress_token),
            progress,
            elicitation,
        )
        .await
    }

    async fn list_resources(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResource>, McpError> {
        let result = self
            .request("resources/list", cursor_parameters(cursor)?)
            .await?;
        parse_resource_page(&result, "resources", |value| {
            Ok(McpResource {
                uri: resource_string(value, "uri", 4096)?,
                name: resource_string(value, "name", 512)?,
                description: optional_resource_string(value, "description", 4096)?
                    .unwrap_or_default(),
                mime_type: optional_resource_string(value, "mimeType", 256)?,
            })
        })
    }

    async fn list_resource_templates(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResourceTemplate>, McpError> {
        let result = self
            .request("resources/templates/list", cursor_parameters(cursor)?)
            .await?;
        parse_resource_page(&result, "resourceTemplates", |value| {
            Ok(McpResourceTemplate {
                uri_template: resource_string(value, "uriTemplate", 4096)?,
                name: resource_string(value, "name", 512)?,
                description: optional_resource_string(value, "description", 4096)?
                    .unwrap_or_default(),
                mime_type: optional_resource_string(value, "mimeType", 256)?,
            })
        })
    }

    async fn read_resource(&self, uri: &str) -> Result<Vec<McpResourceContent>, McpError> {
        validate_resource_value(uri, "resource URI", 4096)?;
        let result = self.request("resources/read", json!({"uri": uri})).await?;
        parse_resource_contents(&result)
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        self.send_message(
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
            true,
        )
        .await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.request_with_progress(method, params, None, None, None)
            .await
    }

    async fn request_with_progress(
        &self,
        method: &str,
        params: Value,
        progress_token: Option<&str>,
        progress_sender: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send_interactive_request(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
            id,
            progress_token,
            progress_sender,
            elicitation,
        )
        .await
    }

    async fn send_message(
        &self,
        message: Value,
        initialized: bool,
    ) -> Result<(HeaderMap, Vec<Value>), McpError> {
        let encoded =
            serde_json::to_vec(&message).map_err(|error| McpError::Protocol(error.to_string()))?;
        if encoded.len() > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol("request exceeds 1 MiB".into()));
        }
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .headers(self.request_headers().await?)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(encoded);
        if initialized {
            request = request.header("mcp-protocol-version", &self.protocol_version);
            if let Some(session_id) = &self.session_id {
                request = request.header("mcp-session-id", session_id);
            }
        }
        let response = request.send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(McpError::AuthenticationRequired);
        }
        if !response.status().is_success() {
            return Err(McpError::Server(format!(
                "HTTP {} from MCP endpoint",
                response.status()
            )));
        }
        let headers = response.headers().clone();
        if response.status() == StatusCode::ACCEPTED || response.status() == StatusCode::NO_CONTENT
        {
            return Ok((headers, Vec::new()));
        }
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let body = bounded_http_body(response).await?;
        let messages = match content_type {
            "application/json" => vec![
                serde_json::from_slice(&body)
                    .map_err(|error| McpError::Protocol(error.to_string()))?,
            ],
            "text/event-stream" => parse_sse_messages(&body)?,
            _ => {
                return Err(McpError::Protocol(format!(
                    "unsupported MCP HTTP content type {content_type:?}"
                )));
            }
        };
        Ok((headers, messages))
    }

    async fn send_interactive_request(
        &self,
        message: Value,
        expected_id: u64,
        progress_token: Option<&str>,
        progress_sender: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        let encoded =
            serde_json::to_vec(&message).map_err(|error| McpError::Protocol(error.to_string()))?;
        if encoded.len() > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol("request exceeds 1 MiB".into()));
        }
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .headers(self.request_headers().await?)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", &self.protocol_version)
            .body(encoded);
        if let Some(session_id) = &self.session_id {
            request = request.header("mcp-session-id", session_id);
        }
        let response = request.send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(McpError::AuthenticationRequired);
        }
        if !response.status().is_success() {
            return Err(McpError::Server(format!(
                "HTTP {} from MCP endpoint",
                response.status()
            )));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if content_type == "application/json" {
            let body = bounded_http_body(response).await?;
            let message: Value = serde_json::from_slice(&body)
                .map_err(|error| McpError::Protocol(error.to_string()))?;
            return response_value(&message, expected_id);
        }
        if content_type != "text/event-stream" {
            return Err(McpError::Protocol(format!(
                "unsupported MCP HTTP content type {content_type:?}"
            )));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut received = 0_usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            received = received.saturating_add(chunk.len());
            if received > MAX_MESSAGE_BYTES {
                return Err(McpError::Protocol("response exceeds 1 MiB".into()));
            }
            buffer.extend_from_slice(&chunk);
            for message in drain_sse_messages(&mut buffer)? {
                if message.get("id").and_then(Value::as_u64) == Some(expected_id) {
                    return response_value(&message, expected_id);
                }
                if message.get("id").is_none()
                    && message.get("method").and_then(Value::as_str)
                        == Some("notifications/progress")
                    && let (Some(expected), Some(sender)) =
                        (progress_token, progress_sender.as_ref())
                    && let Some(update) = parse_progress_notification(&message, expected)?
                {
                    let _ = sender.send(update).await;
                    continue;
                }
                if message.get("method").and_then(Value::as_str) == Some("elicitation/create") {
                    let reply =
                        elicitation_reply(&self.server_id, &message, elicitation.as_ref()).await?;
                    self.send_message(reply, true).await?;
                }
            }
        }
        for message in finish_sse_messages(&mut buffer)? {
            if message.get("id").and_then(Value::as_u64) == Some(expected_id) {
                return response_value(&message, expected_id);
            }
        }
        Err(McpError::Protocol(
            "MCP HTTP stream ended before the response".into(),
        ))
    }

    async fn request_headers(&self) -> Result<HeaderMap, McpError> {
        let mut headers = self.headers.clone();
        let Some(provider) = &self.authorization else {
            return Ok(headers);
        };
        let Some(token) = provider.bearer_token(&self.server_id).await? else {
            return Ok(headers);
        };
        if headers.contains_key(reqwest::header::AUTHORIZATION) {
            return Err(McpError::Configuration(
                "MCP HTTP server cannot combine an OAuth credential with an Authorization header handle"
                    .into(),
            ));
        }
        if token.is_empty() || token.len() > 16 * 1024 {
            return Err(McpError::Configuration(
                "MCP OAuth bearer credential is empty or too large".into(),
            ));
        }
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            McpError::Configuration("MCP OAuth bearer credential is invalid".into())
        })?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
        Ok(headers)
    }
}

impl ConnectedMcpClient {
    async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        match self {
            Self::Stdio(client) => client.list_tools().await,
            Self::Http(client) => client.list_tools().await,
        }
    }

    async fn call_tool_with_progress(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        match self {
            Self::Stdio(client) => {
                client
                    .call_tool_with_progress_and_elicitation(name, arguments, progress, elicitation)
                    .await
            }
            Self::Http(client) => {
                client
                    .call_tool_with_progress(name, arguments, progress, elicitation)
                    .await
            }
        }
    }

    async fn list_resources(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResource>, McpError> {
        match self {
            Self::Stdio(client) => client.list_resources(cursor).await,
            Self::Http(client) => client.list_resources(cursor).await,
        }
    }

    async fn list_resource_templates(
        &self,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResourceTemplate>, McpError> {
        match self {
            Self::Stdio(client) => client.list_resource_templates(cursor).await,
            Self::Http(client) => client.list_resource_templates(cursor).await,
        }
    }

    async fn read_resource(&self, uri: &str) -> Result<Vec<McpResourceContent>, McpError> {
        match self {
            Self::Stdio(client) => client.read_resource(uri).await,
            Self::Http(client) => client.read_resource(uri).await,
        }
    }
}

impl McpRegistry {
    pub async fn connect(configurations: &[McpServerConfig]) -> Result<Self, McpError> {
        Self::connect_with_http(configurations, &[]).await
    }

    pub async fn connect_with_http(
        configurations: &[McpServerConfig],
        http_configurations: &[McpHttpServerConfig],
    ) -> Result<Self, McpError> {
        Self::connect_with_http_authorization(configurations, http_configurations, None).await
    }

    pub async fn connect_with_http_authorization(
        configurations: &[McpServerConfig],
        http_configurations: &[McpHttpServerConfig],
        authorization: Option<Arc<dyn McpHttpAuthorizationProvider>>,
    ) -> Result<Self, McpError> {
        if configurations.len() + http_configurations.len() > 32 {
            return Err(McpError::Configuration(
                "at most 32 MCP servers may be enabled".into(),
            ));
        }
        let mut server_ids = BTreeSet::new();
        let mut registered = BTreeMap::new();
        let mut clients = BTreeMap::new();
        for configuration in configurations {
            if !server_ids.insert(configuration.id.clone()) {
                return Err(McpError::Configuration(
                    "MCP server ids must be unique".into(),
                ));
            }
            let client =
                ConnectedMcpClient::Stdio(Arc::new(McpClient::connect(configuration).await?));
            clients.insert(configuration.id.clone(), client.clone());
            for tool in client.list_tools().await? {
                let name = format!("mcp.{}.{}", configuration.id, tool.name);
                let definition = RegisteredMcpTool {
                    name: name.clone(),
                    description: format!("MCP {}: {}", configuration.id, tool.description),
                    input_schema: tool.input_schema,
                    output_schema: tool.output_schema,
                };
                if registered
                    .insert(
                        name,
                        RegisteredTool {
                            client: client.clone(),
                            upstream_name: tool.name,
                            definition,
                        },
                    )
                    .is_some()
                {
                    return Err(McpError::Configuration(
                        "MCP namespaced tool collision".into(),
                    ));
                }
            }
        }
        for configuration in http_configurations {
            if !server_ids.insert(configuration.id.clone()) {
                return Err(McpError::Configuration(
                    "MCP server ids must be unique".into(),
                ));
            }
            let client = ConnectedMcpClient::Http(Arc::new(
                McpHttpClient::connect_with_authorization(configuration, authorization.clone())
                    .await?,
            ));
            clients.insert(configuration.id.clone(), client.clone());
            for tool in client.list_tools().await? {
                let name = format!("mcp.{}.{}", configuration.id, tool.name);
                let definition = RegisteredMcpTool {
                    name: name.clone(),
                    description: format!("MCP {}: {}", configuration.id, tool.description),
                    input_schema: tool.input_schema,
                    output_schema: tool.output_schema,
                };
                if registered
                    .insert(
                        name,
                        RegisteredTool {
                            client: client.clone(),
                            upstream_name: tool.name,
                            definition,
                        },
                    )
                    .is_some()
                {
                    return Err(McpError::Configuration(
                        "MCP namespaced tool collision".into(),
                    ));
                }
            }
        }
        Ok(Self {
            tools: Arc::new(RwLock::new(registered)),
            clients: Arc::new(RwLock::new(clients)),
        })
    }

    pub fn definitions(&self) -> Vec<RegisteredMcpTool> {
        self.tools
            .read()
            .expect("MCP registry read lock poisoned")
            .values()
            .map(|tool| tool.definition.clone())
            .collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools
            .read()
            .expect("MCP registry read lock poisoned")
            .contains_key(name)
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.call_with_progress(name, arguments, None).await
    }

    pub async fn call_with_progress(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
    ) -> Result<Value, McpError> {
        self.call_with_progress_and_elicitation(name, arguments, progress, None)
            .await
    }

    pub async fn call_with_progress_and_elicitation(
        &self,
        name: &str,
        arguments: Value,
        progress: Option<mpsc::Sender<McpProgressUpdate>>,
        elicitation: Option<Arc<dyn McpElicitationHandler>>,
    ) -> Result<Value, McpError> {
        let tool = self
            .tools
            .read()
            .expect("MCP registry read lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| McpError::Protocol("unknown MCP tool".into()))?;
        tool.client
            .call_tool_with_progress(&tool.upstream_name, arguments, progress, elicitation)
            .await
    }

    pub fn server_ids(&self) -> Vec<String> {
        self.clients
            .read()
            .expect("MCP registry client read lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    pub fn contains_server(&self, server_id: &str) -> bool {
        self.clients
            .read()
            .expect("MCP registry client read lock poisoned")
            .contains_key(server_id)
    }

    pub async fn list_resources(
        &self,
        server_id: &str,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResource>, McpError> {
        self.client(server_id)?.list_resources(cursor).await
    }

    pub async fn list_resource_templates(
        &self,
        server_id: &str,
        cursor: Option<&str>,
    ) -> Result<McpResourcePage<McpResourceTemplate>, McpError> {
        self.client(server_id)?
            .list_resource_templates(cursor)
            .await
    }

    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<Vec<McpResourceContent>, McpError> {
        self.client(server_id)?.read_resource(uri).await
    }

    fn client(&self, server_id: &str) -> Result<ConnectedMcpClient, McpError> {
        self.clients
            .read()
            .expect("MCP registry client read lock poisoned")
            .get(server_id)
            .cloned()
            .ok_or_else(|| McpError::Protocol("unknown MCP server".into()))
    }

    /// Atomically replaces the discoverable runtime set. Existing in-flight calls keep
    /// their process reference, while every subsequent discovery/call sees the new set.
    pub fn replace_with(&self, replacement: &McpRegistry) {
        let next = replacement
            .tools
            .read()
            .expect("replacement MCP registry read lock poisoned")
            .clone();
        *self
            .tools
            .write()
            .expect("MCP registry write lock poisoned") = next;
        let next_clients = replacement
            .clients
            .read()
            .expect("replacement MCP registry client read lock poisoned")
            .clone();
        *self
            .clients
            .write()
            .expect("MCP registry client write lock poisoned") = next_clients;
    }
}

fn parse_tools(result: &Value) -> Result<Vec<McpTool>, McpError> {
    let raw = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::Protocol("tools/list result has no tools array".into()))?;
    if raw.len() > MAX_TOOLS {
        return Err(McpError::Protocol("server returned too many tools".into()));
    }
    let mut names = BTreeSet::new();
    raw.iter()
        .map(|value| {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::Protocol("tool name is missing".into()))?;
            if name.is_empty()
                || name.len() > 128
                || !names.insert(name.to_owned())
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
            {
                return Err(McpError::Protocol(
                    "tool names must be unique, bounded safe identifiers".into(),
                ));
            }
            let input_schema = value
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            if !input_schema.is_object() {
                return Err(McpError::Protocol(
                    "tool inputSchema must be an object".into(),
                ));
            }
            let output_schema = value
                .get("outputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type":"object"}));
            if !output_schema.is_object() {
                return Err(McpError::Protocol(
                    "tool outputSchema must be an object".into(),
                ));
            }
            Ok(McpTool {
                name: name.into(),
                description: value
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .chars()
                    .take(4096)
                    .collect(),
                input_schema,
                output_schema,
            })
        })
        .collect()
}

fn parse_resource_contents(result: &Value) -> Result<Vec<McpResourceContent>, McpError> {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::Protocol("resources/read result has no contents array".into()))?;
    if contents.len() > MAX_RESOURCES {
        return Err(McpError::Protocol(
            "server returned too many resource contents".into(),
        ));
    }
    contents
        .iter()
        .map(|value| {
            let uri = resource_string(value, "uri", 4096)?;
            let mime_type = optional_resource_string(value, "mimeType", 256)?;
            let text = optional_resource_string(value, "text", MAX_MESSAGE_BYTES)?;
            let blob_base64 = optional_resource_string(value, "blob", MAX_MESSAGE_BYTES)?;
            if text.is_some() == blob_base64.is_some() {
                return Err(McpError::Protocol(
                    "resource content must contain exactly one of text or blob".into(),
                ));
            }
            Ok(McpResourceContent {
                uri,
                mime_type,
                text,
                blob_base64,
            })
        })
        .collect()
}

fn response_result(messages: Vec<Value>, expected_id: u64) -> Result<Value, McpError> {
    let response = messages
        .into_iter()
        .find(|message| message.get("id").and_then(Value::as_u64) == Some(expected_id))
        .ok_or_else(|| McpError::Protocol("MCP HTTP response id is missing".into()))?;
    if let Some(error) = response.get("error") {
        return Err(McpError::Server(error.to_string()));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("response result is missing".into()))
}

fn response_value(message: &Value, expected_id: u64) -> Result<Value, McpError> {
    if message.get("id").and_then(Value::as_u64) != Some(expected_id) {
        return Err(McpError::Protocol("MCP HTTP response id mismatch".into()));
    }
    if let Some(error) = message.get("error") {
        return Err(McpError::Server(error.to_string()));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("response result is missing".into()))
}

async fn elicitation_reply(
    server_id: &str,
    message: &Value,
    handler: Option<&Arc<dyn McpElicitationHandler>>,
) -> Result<Value, McpError> {
    let request_id = message
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
        .cloned()
        .ok_or_else(|| McpError::Protocol("elicitation request id is missing".into()))?;
    let params = message
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| McpError::Protocol("elicitation params are missing".into()))?;
    let prompt = params
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("elicitation message is missing".into()))?;
    if prompt.is_empty() || prompt.len() > 4096 || prompt.chars().any(unsafe_control_character) {
        return Err(McpError::Protocol(
            "elicitation message must be bounded safe text".into(),
        ));
    }
    let requested_schema = params
        .get("requestedSchema")
        .filter(|schema| schema.is_object())
        .cloned()
        .ok_or_else(|| {
            McpError::Protocol("elicitation requestedSchema must be an object".into())
        })?;
    if serde_json::to_vec(&requested_schema)
        .map_err(|error| McpError::Protocol(error.to_string()))?
        .len()
        > 64 * 1024
    {
        return Err(McpError::Protocol(
            "elicitation requestedSchema exceeds 64 KiB".into(),
        ));
    }
    let response = match handler {
        Some(handler) => {
            handler
                .elicit(McpElicitationRequest {
                    request_id: request_id.clone(),
                    server_id: server_id.into(),
                    message: prompt.into(),
                    requested_schema,
                })
                .await?
        }
        None => McpElicitationResponse::Cancel,
    };
    let result =
        serde_json::to_value(response).map_err(|error| McpError::Protocol(error.to_string()))?;
    Ok(json!({"jsonrpc": "2.0", "id": request_id, "result": result}))
}

async fn bounded_http_body(response: reqwest::Response) -> Result<Vec<u8>, McpError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > MAX_MESSAGE_BYTES {
            return Err(McpError::Protocol("response exceeds 1 MiB".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_sse_messages(body: &[u8]) -> Result<Vec<Value>, McpError> {
    let mut buffer = body.to_vec();
    let mut messages = drain_sse_messages(&mut buffer)?;
    messages.extend(finish_sse_messages(&mut buffer)?);
    if messages.is_empty() {
        return Err(McpError::Protocol(
            "MCP SSE response contained no JSON-RPC messages".into(),
        ));
    }
    Ok(messages)
}

fn drain_sse_messages(buffer: &mut Vec<u8>) -> Result<Vec<Value>, McpError> {
    let mut messages = Vec::new();
    loop {
        let boundary = buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|index| (index, 2))
            .or_else(|| {
                buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| (index, 4))
            });
        let Some((index, delimiter_length)) = boundary else {
            break;
        };
        let frame = buffer.drain(..index).collect::<Vec<_>>();
        buffer.drain(..delimiter_length);
        if let Some(message) = parse_sse_frame(&frame)? {
            messages.push(message);
        }
    }
    Ok(messages)
}

fn finish_sse_messages(buffer: &mut Vec<u8>) -> Result<Vec<Value>, McpError> {
    if buffer.is_empty() {
        return Ok(Vec::new());
    }
    let frame = std::mem::take(buffer);
    Ok(parse_sse_frame(&frame)?.into_iter().collect())
}

fn parse_sse_frame(frame: &[u8]) -> Result<Option<Value>, McpError> {
    let text = std::str::from_utf8(frame)
        .map_err(|_| McpError::Protocol("MCP SSE response is not UTF-8".into()))?;
    let mut data = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| McpError::Protocol(error.to_string()))
}

fn parse_progress_notification(
    response: &Value,
    expected_token: &str,
) -> Result<Option<McpProgressUpdate>, McpError> {
    let params = response
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| McpError::Protocol("progress notification params are missing".into()))?;
    let token_matches = match params.get("progressToken") {
        Some(Value::String(value)) => value == expected_token,
        Some(Value::Number(value)) => value.to_string() == expected_token,
        Some(_) => {
            return Err(McpError::Protocol(
                "progressToken must be a string or number".into(),
            ));
        }
        None => {
            return Err(McpError::Protocol(
                "progress notification has no progressToken".into(),
            ));
        }
    };
    if !token_matches {
        return Ok(None);
    }
    let progress = params
        .get("progress")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| {
            McpError::Protocol("progress must be a finite non-negative number".into())
        })?;
    let total = params
        .get("total")
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| {
                    McpError::Protocol("progress total must be a finite non-negative number".into())
                })
        })
        .transpose()?;
    if total.is_some_and(|total| progress > total) {
        return Err(McpError::Protocol(
            "progress cannot exceed its total".into(),
        ));
    }
    let message = params
        .get("message")
        .map(|value| {
            let message = value
                .as_str()
                .ok_or_else(|| McpError::Protocol("progress message must be a string".into()))?;
            if message.len() > MAX_PROGRESS_MESSAGE_BYTES
                || message.chars().any(unsafe_control_character)
            {
                return Err(McpError::Protocol(
                    "progress message must be bounded and contain no unsafe controls".into(),
                ));
            }
            Ok(message.to_owned())
        })
        .transpose()?;
    Ok(Some(McpProgressUpdate {
        progress,
        total,
        message,
    }))
}

fn cursor_parameters(cursor: Option<&str>) -> Result<Value, McpError> {
    match cursor {
        Some(cursor) => {
            validate_resource_value(cursor, "resource cursor", 4096)?;
            Ok(json!({"cursor": cursor}))
        }
        None => Ok(json!({})),
    }
}

fn parse_resource_page<T>(
    result: &Value,
    key: &str,
    parse: impl Fn(&Value) -> Result<T, McpError>,
) -> Result<McpResourcePage<T>, McpError> {
    let raw = result
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::Protocol(format!("{key} result has no {key} array")))?;
    if raw.len() > MAX_RESOURCES {
        return Err(McpError::Protocol(
            "server returned too many resources".into(),
        ));
    }
    let next_cursor = result
        .get("nextCursor")
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| McpError::Protocol("resource nextCursor is not a string".into()))?;
            validate_resource_value(value, "resource nextCursor", 4096)?;
            Ok::<String, McpError>(value.to_owned())
        })
        .transpose()?;
    Ok(McpResourcePage {
        entries: raw.iter().map(parse).collect::<Result<_, _>>()?,
        next_cursor,
    })
}

fn resource_string(value: &Value, key: &str, max_bytes: usize) -> Result<String, McpError> {
    let value = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol(format!("resource {key} is missing")))?;
    validate_resource_value(value, &format!("resource {key}"), max_bytes)?;
    Ok(value.to_owned())
}

fn optional_resource_string(
    value: &Value,
    key: &str,
    max_bytes: usize,
) -> Result<Option<String>, McpError> {
    value
        .get(key)
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| McpError::Protocol(format!("resource {key} is not a string")))?;
            if value.len() > max_bytes || value.chars().any(unsafe_control_character) {
                return Err(McpError::Protocol(format!(
                    "resource {key} must be bounded and contain no control characters"
                )));
            }
            Ok(value.to_owned())
        })
        .transpose()
}

fn validate_resource_value(value: &str, label: &str, max_bytes: usize) -> Result<(), McpError> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(McpError::Protocol(format!(
            "{label} must be non-empty, bounded and contain no control characters"
        )));
    }
    Ok(())
}

fn unsafe_control_character(value: char) -> bool {
    value.is_control() && !matches!(value, '\n' | '\r' | '\t')
}

fn valid_environment_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_progress_token_is_ignored() {
        let notification = json!({
            "jsonrpc":"2.0",
            "method":"notifications/progress",
            "params":{
                "progressToken":"another-request",
                "progress":50,
                "total":100
            }
        });
        assert_eq!(
            parse_progress_notification(&notification, "expected").unwrap(),
            None
        );
    }

    #[test]
    fn matching_malformed_progress_fails_closed() {
        let notification = json!({
            "jsonrpc":"2.0",
            "method":"notifications/progress",
            "params":{
                "progressToken":"expected",
                "progress":101,
                "total":100
            }
        });
        assert!(matches!(
            parse_progress_notification(&notification, "expected"),
            Err(McpError::Protocol(_))
        ));
    }
}
