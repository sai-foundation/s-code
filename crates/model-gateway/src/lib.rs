pub mod privacy;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: String,
    pub content: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub temperature: f32,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolDefinition>,
    pub max_output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ModelRoutingPolicy>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    RateLimited,
    Timeout,
    ProviderUnavailable,
    ContextOverflow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoutingPolicy {
    pub routing_order: Vec<String>,
    pub fallback_reasons: Vec<FallbackReason>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    TextDelta {
        text: String,
    },
    ReasoningSummaryDelta {
        text: String,
    },
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<Value>,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Completed {
        finish_reason: Option<String>,
    },
    RouteSelected {
        model_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback_from: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<FallbackReason>,
    },
    RouteFallback {
        from_model_id: String,
        to_model_id: String,
        reason: FallbackReason,
    },
}

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("credential unavailable: {0}")]
    Credential(String),
    #[error("provider request failed: {0}")]
    Provider(String),
    #[error("provider rate limited: {0}")]
    RateLimited(String),
    #[error("provider timed out: {0}")]
    Timeout(String),
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    #[error("provider context overflow: {0}")]
    ContextOverflow(String),
    #[error("invalid provider response: {0}")]
    InvalidResponse(String),
}

pub type ModelStream =
    Pin<Box<dyn futures_util::Stream<Item = Result<ModelEvent, GatewayError>> + Send>>;

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    async fn resolve(&self, handle: &str) -> Result<String, GatewayError>;
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError>;
}

impl GatewayError {
    pub fn fallback_reason(&self) -> Option<FallbackReason> {
        match self {
            Self::RateLimited(_) => Some(FallbackReason::RateLimited),
            Self::Timeout(_) => Some(FallbackReason::Timeout),
            Self::Unavailable(_) => Some(FallbackReason::ProviderUnavailable),
            Self::ContextOverflow(_) => Some(FallbackReason::ContextOverflow),
            Self::Credential(_) | Self::Provider(_) | Self::InvalidResponse(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct RoutedModelEndpoint {
    pub id: String,
    pub provider_model: String,
    pub provider: Arc<dyn ModelProvider>,
}

pub struct GovernedModelRouter {
    endpoints: BTreeMap<String, RoutedModelEndpoint>,
}

impl GovernedModelRouter {
    pub fn new(endpoints: Vec<RoutedModelEndpoint>) -> Result<Self, GatewayError> {
        if endpoints.is_empty() || endpoints.len() > 64 {
            return Err(GatewayError::InvalidResponse(
                "model router requires 1 to 64 endpoints".into(),
            ));
        }
        let mut indexed = BTreeMap::new();
        for endpoint in endpoints {
            if !bounded_identifier(&endpoint.id)
                || endpoint.provider_model.trim().is_empty()
                || endpoint.provider_model.len() > 256
                || indexed.insert(endpoint.id.clone(), endpoint).is_some()
            {
                return Err(GatewayError::InvalidResponse(
                    "model router endpoint is invalid or duplicated".into(),
                ));
            }
        }
        Ok(Self { endpoints: indexed })
    }

    fn route(&self, request: &ModelRequest) -> Result<ModelRoutingPolicy, GatewayError> {
        let route = request
            .routing
            .clone()
            .unwrap_or_else(|| ModelRoutingPolicy {
                routing_order: vec![request.model.clone()],
                fallback_reasons: Vec::new(),
            });
        let unique = route.routing_order.iter().collect::<BTreeSet<_>>();
        let reasons = route.fallback_reasons.iter().collect::<BTreeSet<_>>();
        if route.routing_order.is_empty()
            || route.routing_order.len() > 64
            || unique.len() != route.routing_order.len()
            || reasons.len() != route.fallback_reasons.len()
            || route.routing_order.first() != Some(&request.model)
            || route
                .routing_order
                .iter()
                .any(|model| !self.endpoints.contains_key(model))
        {
            return Err(GatewayError::InvalidResponse(
                "model routing policy is invalid or references an unavailable endpoint".into(),
            ));
        }
        Ok(route)
    }
}

#[async_trait]
impl ModelProvider for GovernedModelRouter {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let route = self.route(&request)?;
        let allowed = route
            .fallback_reasons
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut fallback_from = None;
        let mut fallback_reason = None;
        let mut routing_events = Vec::new();
        for (index, model_id) in route.routing_order.iter().enumerate() {
            let endpoint = self.endpoints.get(model_id).expect("validated route");
            let mut routed_request = request.clone();
            routed_request.model = endpoint.provider_model.clone();
            routed_request.routing = None;
            match endpoint.provider.stream(routed_request).await {
                Ok(stream) => {
                    let selected = ModelEvent::RouteSelected {
                        model_id: model_id.clone(),
                        fallback_from,
                        reason: fallback_reason,
                    };
                    routing_events.push(selected);
                    return Ok(Box::pin(
                        futures_util::stream::iter(routing_events.into_iter().map(Ok))
                            .chain(stream),
                    ));
                }
                Err(error) => {
                    let Some(reason) = error.fallback_reason() else {
                        return Err(error);
                    };
                    if !allowed.contains(&reason) || index + 1 == route.routing_order.len() {
                        return Err(error);
                    }
                    routing_events.push(ModelEvent::RouteFallback {
                        from_model_id: model_id.clone(),
                        to_model_id: route.routing_order[index + 1].clone(),
                        reason: reason.clone(),
                    });
                    fallback_from = Some(model_id.clone());
                    fallback_reason = Some(reason);
                }
            }
        }
        Err(GatewayError::InvalidResponse(
            "model route unexpectedly exhausted".into(),
        ))
    }
}

fn bounded_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
}

fn request_error(error: reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::Timeout(error.to_string())
    } else if error.is_connect() {
        GatewayError::Unavailable(error.to_string())
    } else {
        GatewayError::Provider(error.to_string())
    }
}

fn status_error(status: reqwest::StatusCode) -> GatewayError {
    match status.as_u16() {
        408 | 504 => GatewayError::Timeout(status.to_string()),
        429 => GatewayError::RateLimited(status.to_string()),
        413 => GatewayError::ContextOverflow(status.to_string()),
        502 | 503 => GatewayError::Unavailable(status.to_string()),
        _ => GatewayError::Provider(status.to_string()),
    }
}

pub struct EnvironmentCredentials;
#[async_trait]
impl CredentialProvider for EnvironmentCredentials {
    async fn resolve(&self, handle: &str) -> Result<String, GatewayError> {
        std::env::var(handle).map_err(|_| GatewayError::Credential(handle.into()))
    }
}

pub struct OpenAiCompatible {
    client: reqwest::Client,
    base_url: String,
    credential_handle: Option<String>,
    credentials: Arc<dyn CredentialProvider>,
}

pub struct AnthropicMessages {
    client: reqwest::Client,
    base_url: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialProvider>,
}

pub struct GeminiGenerateContent {
    client: reqwest::Client,
    base_url: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialProvider>,
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(45))
        .timeout(Duration::from_secs(180))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static model gateway HTTP configuration must be valid")
}

impl OpenAiCompatible {
    pub fn new(
        base_url: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            client: http_client(),
            base_url: base_url.into().trim_end_matches('/').into(),
            credential_handle: Some(credential_handle.into()),
            credentials,
        }
    }

    pub fn without_auth(
        base_url: impl Into<String>,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            client: http_client(),
            base_url: base_url.into().trim_end_matches('/').into(),
            credential_handle: None,
            credentials,
        }
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatible {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let tools: Vec<Value> = request.tools.iter().map(|t| serde_json::json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect();
        let messages = openai_messages(&request.messages)?;
        let mut builder = self
            .client
            .post(format!("{}/chat/completions", self.base_url));
        if let Some(handle) = self.credential_handle.as_deref() {
            let key = self.credentials.resolve(handle).await?;
            builder = builder.bearer_auth(key);
        }
        let body = serde_json::json!({"model":request.model,"temperature":request.temperature,"messages":messages,"tools":tools,"max_tokens":request.max_output_tokens,"stream":true,"stream_options":{"include_usage":true}});
        let response = privacy::send(builder, &self.base_url, &request, &body).await?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        Ok(defer_openai_completion(sse_stream(
            response,
            parse_sse_line,
        )))
    }
}

impl AnthropicMessages {
    pub fn new(
        base_url: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            client: http_client(),
            base_url: base_url.into().trim_end_matches('/').into(),
            credential_handle: credential_handle.into(),
            credentials,
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicMessages {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let key = self.credentials.resolve(&self.credential_handle).await?;
        let (system, messages) = anthropic_messages(&request.messages)?;
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                })
            })
            .collect();
        let mut body = serde_json::json!({
            "model": request.model,
            "temperature": request.temperature,
            "messages": messages,
            "tools": tools,
            "max_tokens": request.max_output_tokens,
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = Value::String(system);
        }
        let builder = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01");
        let response = privacy::send(builder, &self.base_url, &request, &body).await?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        Ok(sse_stream(response, parse_anthropic_sse_line))
    }
}

impl GeminiGenerateContent {
    pub fn new(
        base_url: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialProvider>,
    ) -> Self {
        Self {
            client: http_client(),
            base_url: base_url.into().trim_end_matches('/').into(),
            credential_handle: credential_handle.into(),
            credentials,
        }
    }
}

#[async_trait]
impl ModelProvider for GeminiGenerateContent {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        if request.model.is_empty()
            || !request
                .model
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(GatewayError::InvalidResponse(
                "Gemini model ID contains unsupported path characters".into(),
            ));
        }
        let key = self.credentials.resolve(&self.credential_handle).await?;
        let (system_instruction, contents) = gemini_messages(&request.messages)?;
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();
        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "temperature": request.temperature,
                "maxOutputTokens": request.max_output_tokens,
            },
        });
        if !declarations.is_empty() {
            body["tools"] = serde_json::json!([{"functionDeclarations": declarations}]);
        }
        if !system_instruction.is_empty() {
            body["systemInstruction"] =
                serde_json::json!({"parts": [{"text": system_instruction}]});
        }
        let builder = self
            .client
            .post(format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, request.model
            ))
            .header("x-goog-api-key", key);
        let response = privacy::send(builder, &self.base_url, &request, &body).await?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        Ok(sse_stream(response, parse_gemini_sse_line))
    }
}

const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;
const MAX_SSE_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SSE_MODEL_EVENTS: usize = 16_384;

fn sse_stream(
    response: reqwest::Response,
    parser: fn(&str) -> Result<Vec<ModelEvent>, GatewayError>,
) -> ModelStream {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SSE_RESPONSE_BYTES as u64)
    {
        return Box::pin(futures_util::stream::once(async {
            Err(GatewayError::Provider(
                "model stream exceeds the 16 MiB response limit".into(),
            ))
        }));
    }
    let bytes = response.bytes_stream();
    let events = futures_util::stream::try_unfold(
        (bytes, Vec::new(), VecDeque::new(), 0_usize, 0_usize),
        move |(mut bytes, mut buffer, mut pending, mut received, mut emitted)| async move {
            if let Some(event) = pending.pop_front() {
                emitted = emitted.checked_add(1).ok_or_else(|| {
                    GatewayError::Provider("model stream event count overflow".into())
                })?;
                if emitted > MAX_SSE_MODEL_EVENTS {
                    return Err(GatewayError::Provider(
                        "model stream exceeds the event count limit".into(),
                    ));
                }
                return Ok(Some((event, (bytes, buffer, pending, received, emitted))));
            }
            loop {
                if let Some(pos) = buffer.iter().position(|byte| *byte == b'\n') {
                    if pos > MAX_SSE_LINE_BYTES {
                        return Err(GatewayError::Provider(
                            "model stream contains an oversized SSE line".into(),
                        ));
                    }
                    let mut line = buffer.drain(..=pos).collect::<Vec<_>>();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let line = std::str::from_utf8(&line).map_err(|_| {
                        GatewayError::Provider("model stream contains invalid UTF-8".into())
                    })?;
                    pending.extend(parser(line)?);
                    if let Some(event) = pending.pop_front() {
                        emitted = emitted.checked_add(1).ok_or_else(|| {
                            GatewayError::Provider("model stream event count overflow".into())
                        })?;
                        if emitted > MAX_SSE_MODEL_EVENTS {
                            return Err(GatewayError::Provider(
                                "model stream exceeds the event count limit".into(),
                            ));
                        }
                        return Ok(Some((event, (bytes, buffer, pending, received, emitted))));
                    }
                    continue;
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => {
                        received = received.checked_add(chunk.len()).ok_or_else(|| {
                            GatewayError::Provider("model stream size overflow".into())
                        })?;
                        if received > MAX_SSE_RESPONSE_BYTES {
                            return Err(GatewayError::Provider(
                                "model stream exceeds the 16 MiB response limit".into(),
                            ));
                        }
                        buffer.extend_from_slice(&chunk);
                        if buffer.len() > MAX_SSE_LINE_BYTES && !buffer.contains(&b'\n') {
                            return Err(GatewayError::Provider(
                                "model stream contains an oversized SSE line".into(),
                            ));
                        }
                    }
                    Some(Err(error)) => return Err(GatewayError::Provider(error.to_string())),
                    None => {
                        if buffer.is_empty() {
                            return Ok(None);
                        }
                        if buffer.len() > MAX_SSE_LINE_BYTES {
                            return Err(GatewayError::Provider(
                                "model stream contains an oversized SSE line".into(),
                            ));
                        }
                        if buffer.last() == Some(&b'\r') {
                            buffer.pop();
                        }
                        let line = std::str::from_utf8(&buffer).map_err(|_| {
                            GatewayError::Provider("model stream contains invalid UTF-8".into())
                        })?;
                        pending.extend(parser(line)?);
                        buffer.clear();
                        let Some(event) = pending.pop_front() else {
                            return Ok(None);
                        };
                        emitted = emitted.checked_add(1).ok_or_else(|| {
                            GatewayError::Provider("model stream event count overflow".into())
                        })?;
                        if emitted > MAX_SSE_MODEL_EVENTS {
                            return Err(GatewayError::Provider(
                                "model stream exceeds the event count limit".into(),
                            ));
                        }
                        return Ok(Some((event, (bytes, buffer, pending, received, emitted))));
                    }
                }
            }
        },
    );
    Box::pin(events)
}

fn defer_openai_completion(stream: ModelStream) -> ModelStream {
    let events = futures_util::stream::try_unfold(
        (stream, None),
        |(mut stream, mut completion): (ModelStream, Option<ModelEvent>)| async move {
            loop {
                match stream.next().await {
                    Some(Ok(
                        event @ ModelEvent::Completed {
                            finish_reason: Some(_),
                        },
                    )) => {
                        completion = Some(event);
                    }
                    Some(Ok(
                        event @ ModelEvent::Completed {
                            finish_reason: None,
                        },
                    )) => {
                        return Ok(Some((
                            completion.take().unwrap_or(event),
                            (stream, completion),
                        )));
                    }
                    Some(Ok(event)) => return Ok(Some((event, (stream, completion)))),
                    Some(Err(error)) => return Err(error),
                    None => {
                        return Ok(completion.take().map(|event| (event, (stream, completion))));
                    }
                }
            }
        },
    );
    Box::pin(events)
}

/// Translate the provider-neutral internal representation into the actual
/// OpenAI chat schema. Tool calls and their results are top-level message
/// fields, not objects nested inside `content`.
fn openai_messages(messages: &[ModelMessage]) -> Result<Vec<Value>, GatewayError> {
    messages
        .iter()
        .map(|message| match (message.role.as_str(), &message.content) {
            ("assistant", Value::Object(content)) if content.contains_key("tool_calls") => {
                let text = content.get("text").and_then(Value::as_str).unwrap_or("");
                Ok(serde_json::json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { Value::String(text.into()) },
                    "tool_calls": openai_tool_calls(&message.content)?,
                }))
            }
            ("tool", Value::Object(content)) => {
                let call_id = content
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| GatewayError::InvalidResponse("tool message is missing tool_call_id".into()))?;
                let result = content.get("result").cloned().unwrap_or(Value::Null);
                let encoded = serde_json::to_string(&result)
                    .map_err(|error| GatewayError::InvalidResponse(error.to_string()))?;
                Ok(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": encoded,
                }))
            }
            _ => {
                let content = match &message.content {
                    Value::String(_) | Value::Array(_) | Value::Null => message.content.clone(),
                    value => Value::String(
                        serde_json::to_string(value)
                            .map_err(|error| GatewayError::InvalidResponse(error.to_string()))?,
                    ),
                };
                Ok(serde_json::json!({"role": message.role, "content": content}))
            }
        })
        .collect()
}

/// Keep internal replay metadata out of the OpenAI wire protocol. Other
/// adapters consume their own metadata separately (for example Gemini signatures).
fn openai_tool_calls(content: &Value) -> Result<Vec<Value>, GatewayError> {
    let calls = content["tool_calls"].as_array().ok_or_else(|| {
        GatewayError::InvalidResponse("assistant tool calls must be an array".into())
    })?;
    calls
        .iter()
        .map(|call| {
            let id = call["id"]
                .as_str()
                .ok_or_else(|| GatewayError::InvalidResponse("tool call is missing id".into()))?;
            let name = call["function"]["name"]
                .as_str()
                .ok_or_else(|| GatewayError::InvalidResponse("tool call is missing name".into()))?;
            let arguments = call["function"]["arguments"].as_str().ok_or_else(|| {
                GatewayError::InvalidResponse("tool call is missing arguments".into())
            })?;
            Ok(serde_json::json!({
                "id": id, "type": "function", "function": { "name": name, "arguments": arguments }
            }))
        })
        .collect()
}

fn text_content(value: &Value) -> Result<String, GatewayError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        other => serde_json::to_string(other)
            .map_err(|error| GatewayError::InvalidResponse(error.to_string())),
    }
}

struct NormalizedToolCall {
    id: String,
    name: String,
    arguments: Value,
    provider_metadata: Option<Value>,
}

fn tool_calls(content: &Value) -> Result<Vec<NormalizedToolCall>, GatewayError> {
    let calls = content
        .get("tool_calls")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            GatewayError::InvalidResponse("assistant tool calls must be an array".into())
        })?;
    calls
        .iter()
        .map(|call| {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::InvalidResponse("tool call is missing id".into()))?;
            let function = call.get("function").ok_or_else(|| {
                GatewayError::InvalidResponse("tool call is missing function".into())
            })?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| GatewayError::InvalidResponse("tool call is missing name".into()))?;
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    GatewayError::InvalidResponse("tool call is missing arguments".into())
                })?;
            let arguments = serde_json::from_str(arguments).map_err(|error| {
                GatewayError::InvalidResponse(format!("tool arguments are invalid JSON: {error}"))
            })?;
            Ok(NormalizedToolCall {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments,
                provider_metadata: call
                    .get("provider_metadata")
                    .cloned()
                    .filter(|value| !value.is_null()),
            })
        })
        .collect()
}

fn anthropic_messages(messages: &[ModelMessage]) -> Result<(String, Vec<Value>), GatewayError> {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        match (message.role.as_str(), &message.content) {
            ("system", content) => system.push(text_content(content)?),
            ("assistant", Value::Object(content)) if content.contains_key("tool_calls") => {
                let mut blocks = Vec::new();
                if let Some(text) = content
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    blocks.push(serde_json::json!({"type": "text", "text": text}));
                }
                for call in tool_calls(&message.content)? {
                    blocks.push(serde_json::json!({
                        "type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments
                    }));
                }
                converted.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            ("tool", Value::Object(content)) => {
                let id = content
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::InvalidResponse("tool message is missing tool_call_id".into())
                    })?;
                let result = text_content(content.get("result").unwrap_or(&Value::Null))?;
                converted.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": id, "content": result}]
                }));
            }
            ("user" | "assistant", content) => converted.push(serde_json::json!({
                "role": message.role,
                "content": text_content(content)?,
            })),
            (role, _) => {
                return Err(GatewayError::InvalidResponse(format!(
                    "Anthropic does not support internal message role {role}"
                )));
            }
        }
    }
    Ok((system.join("\n\n"), converted))
}

fn gemini_messages(messages: &[ModelMessage]) -> Result<(String, Vec<Value>), GatewayError> {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        match (message.role.as_str(), &message.content) {
            ("system", content) => system.push(text_content(content)?),
            ("assistant", Value::Object(content)) if content.contains_key("tool_calls") => {
                let mut parts = Vec::new();
                if let Some(text) = content
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    parts.push(serde_json::json!({"text": text}));
                }
                for call in tool_calls(&message.content)? {
                    let mut part = serde_json::json!({
                        "functionCall": {"id": call.id, "name": call.name, "args": call.arguments}
                    });
                    if let Some(signature) = call.provider_metadata {
                        part["thoughtSignature"] = signature;
                    }
                    parts.push(part);
                }
                converted.push(serde_json::json!({"role": "model", "parts": parts}));
            }
            ("tool", Value::Object(content)) => {
                let id = content
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        GatewayError::InvalidResponse("tool message is missing tool_call_id".into())
                    })?;
                let name = content.get("name").and_then(Value::as_str).ok_or_else(|| {
                    GatewayError::InvalidResponse("tool message is missing name".into())
                })?;
                let result = content.get("result").cloned().unwrap_or(Value::Null);
                converted.push(serde_json::json!({
                    "role": "user",
                    "parts": [{"functionResponse": {
                        "id": id, "name": name, "response": {"result": result}
                    }}]
                }));
            }
            ("user", content) => converted.push(serde_json::json!({
                "role": "user", "parts": [{"text": text_content(content)?}]
            })),
            ("assistant", content) => converted.push(serde_json::json!({
                "role": "model", "parts": [{"text": text_content(content)?}]
            })),
            (role, _) => {
                return Err(GatewayError::InvalidResponse(format!(
                    "Gemini does not support internal message role {role}"
                )));
            }
        }
    }
    Ok((system.join("\n\n"), converted))
}

fn sse_data(line: &str) -> Result<Option<Value>, GatewayError> {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Ok(None);
    };
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(data)
        .map(Some)
        .map_err(|error| GatewayError::InvalidResponse(error.to_string()))
}

fn parse_anthropic_sse_line(line: &str) -> Result<Vec<ModelEvent>, GatewayError> {
    let Some(value) = sse_data(line)? else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    match value.get("type").and_then(Value::as_str) {
        Some("message_start") => {
            if let Some(usage) = value.get("message").and_then(|v| v.get("usage")) {
                events.push(ModelEvent::Usage {
                    input_tokens: usage
                        .get("input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    output_tokens: usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                });
            }
        }
        Some("content_block_start") => {
            let block = &value["content_block"];
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                events.push(ModelEvent::ToolCallDelta {
                    index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                    id: block.get("id").and_then(Value::as_str).map(str::to_owned),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .map(normalize_tool_name),
                    arguments_delta: String::new(),
                    provider_metadata: None,
                });
            }
        }
        Some("content_block_delta") => match value["delta"].get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                if let Some(text) = value["delta"].get("text").and_then(Value::as_str) {
                    events.push(ModelEvent::TextDelta { text: text.into() });
                }
            }
            Some("input_json_delta") => events.push(ModelEvent::ToolCallDelta {
                index: value.get("index").and_then(Value::as_u64).unwrap_or(0) as u32,
                id: None,
                name: None,
                arguments_delta: value["delta"]
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
                provider_metadata: None,
            }),
            _ => {}
        },
        Some("message_delta") => {
            if let Some(usage) = value.get("usage") {
                events.push(ModelEvent::Usage {
                    input_tokens: 0,
                    output_tokens: usage
                        .get("output_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                });
            }
            events.push(ModelEvent::Completed {
                finish_reason: value["delta"]
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
        Some("error") => {
            return Err(GatewayError::Provider(
                value["error"]
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("Anthropic stream error")
                    .into(),
            ));
        }
        _ => {}
    }
    Ok(events)
}

fn parse_gemini_sse_line(line: &str) -> Result<Vec<ModelEvent>, GatewayError> {
    let Some(value) = sse_data(line)? else {
        return Ok(Vec::new());
    };
    let mut events = Vec::new();
    if let Some(parts) = value["candidates"][0]["content"]["parts"].as_array() {
        for (index, part) in parts.iter().enumerate() {
            if let Some(text) = part
                .get("text")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    events.push(ModelEvent::ReasoningSummaryDelta { text: text.into() });
                } else {
                    events.push(ModelEvent::TextDelta { text: text.into() });
                }
            }
            if let Some(call) = part.get("functionCall") {
                let response_id = value
                    .get("responseId")
                    .and_then(Value::as_str)
                    .unwrap_or("response");
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("gemini-{response_id}-{index}"));
                let arguments_delta = serde_json::to_string(
                    call.get("args")
                        .unwrap_or(&Value::Object(Default::default())),
                )
                .map_err(|error| GatewayError::InvalidResponse(error.to_string()))?;
                events.push(ModelEvent::ToolCallDelta {
                    index: index as u32,
                    id: Some(id),
                    name: call
                        .get("name")
                        .and_then(Value::as_str)
                        .map(normalize_tool_name),
                    arguments_delta,
                    provider_metadata: part.get("thoughtSignature").cloned(),
                });
            }
        }
    }
    if let Some(usage) = value.get("usageMetadata") {
        events.push(ModelEvent::Usage {
            input_tokens: usage
                .get("promptTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: usage
                .get("candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    if let Some(reason) = value["candidates"][0]
        .get("finishReason")
        .and_then(Value::as_str)
    {
        events.push(ModelEvent::Completed {
            finish_reason: Some(reason.into()),
        });
    }
    Ok(events)
}

fn parse_sse_line(line: &str) -> Result<Vec<ModelEvent>, GatewayError> {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Ok(Vec::new());
    };
    if data == "[DONE]" {
        return Ok(vec![ModelEvent::Completed {
            finish_reason: None,
        }]);
    }
    let value: Value =
        serde_json::from_str(data).map_err(|e| GatewayError::InvalidResponse(e.to_string()))?;
    let mut events = Vec::new();
    if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
        events.push(ModelEvent::Usage {
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        });
    }
    let choice = &value["choices"][0];
    let reasoning_details = choice["delta"]["reasoning_details"].as_array();
    if let Some(details) = reasoning_details {
        let summary = details
            .iter()
            .filter(|detail| {
                detail.get("type").and_then(Value::as_str) == Some("reasoning.summary")
            })
            .filter_map(|detail| detail.get("summary").and_then(Value::as_str))
            .collect::<String>();
        if !summary.is_empty() {
            events.push(ModelEvent::ReasoningSummaryDelta { text: summary });
        }
    }
    if let Some(text) = choice["delta"]["content"].as_str() {
        events.push(ModelEvent::TextDelta { text: text.into() });
    }
    if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
        for call in calls {
            events.push(ModelEvent::ToolCallDelta {
                index: call["index"].as_u64().unwrap_or(0) as u32,
                id: call["id"].as_str().map(str::to_string),
                name: call["function"]["name"].as_str().map(normalize_tool_name),
                arguments_delta: call["function"]["arguments"].as_str().unwrap_or("").into(),
                provider_metadata: None,
            });
        }
    }
    let has_private_reasoning = reasoning_details.is_some_and(|details| !details.is_empty())
        || choice["delta"]["reasoning"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
        || choice["delta"]["reasoning_content"]
            .as_str()
            .is_some_and(|text| !text.is_empty());
    if has_private_reasoning && events.is_empty() {
        events.push(ModelEvent::TextDelta {
            text: String::new(),
        });
    }
    if !choice["finish_reason"].is_null() {
        events.push(ModelEvent::Completed {
            finish_reason: choice["finish_reason"].as_str().map(str::to_string),
        });
    }
    Ok(events)
}

fn normalize_tool_name(name: &str) -> String {
    const CHANNEL_SUFFIXES: [&str; 3] = [
        "<|channel|>analysis",
        "<|channel|>commentary",
        "<|channel|>final",
    ];
    CHANNEL_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::{Body, Bytes},
        extract::State,
        http::{HeaderMap, StatusCode},
        response::Response,
        routing::{get, post},
    };
    use futures_util::StreamExt;
    use serde_json::json;
    use std::convert::Infallible;
    use std::sync::Mutex;

    struct FixedCredentials;

    #[async_trait]
    impl CredentialProvider for FixedCredentials {
        async fn resolve(&self, handle: &str) -> Result<String, GatewayError> {
            assert_eq!(handle, "provider-primary");
            Ok("short-lived-secret".into())
        }
    }

    #[test]
    fn openai_history_excludes_internal_metadata_and_preserves_call_arguments() {
        for metadata in [Value::Null, serde_json::json!("opaque-provider-signature")] {
            let arguments = "{ \"query\": \"weather 天气 forecast\", \"limit\": 8 }";
            let message = ModelMessage {
                role: "assistant".into(),
                content: serde_json::json!({
                    "text": "", "tool_calls": [{ "id": "call_search", "type": "function",
                        "function": {"name": "tool_search", "arguments": arguments},
                        "provider_metadata": metadata,
                    }],
                }),
            };
            let output = openai_messages(std::slice::from_ref(&message)).unwrap();
            assert_eq!(
                output[0]["tool_calls"][0],
                serde_json::json!({
                    "id": "call_search", "type": "function", "function": {
                        "name": "tool_search", "arguments": arguments,
                    }
                })
            );
            assert_eq!(
                message.content["tool_calls"][0]["provider_metadata"], metadata,
                "adapter projection must not mutate internal history"
            );
            assert!(output[0]["content"].is_null());
        }
    }

    #[test]
    fn parses_text_delta() {
        let events = parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(events, vec![ModelEvent::TextDelta { text: "hi".into() }]);
    }

    #[test]
    fn parses_empty_text_delta_as_stream_liveness() {
        let events =
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":""},"finish_reason":null}]}"#)
                .unwrap();
        assert_eq!(
            events,
            vec![ModelEvent::TextDelta {
                text: String::new()
            }]
        );
    }

    #[test]
    fn exposes_only_reasoning_summaries_but_preserves_private_reasoning_liveness() {
        let events = parse_sse_line(
            r#"data: {"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.summary","summary":"Checked the constraints. "},{"type":"reasoning.text","text":"private raw reasoning"}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            events,
            vec![ModelEvent::ReasoningSummaryDelta {
                text: "Checked the constraints. ".into()
            }]
        );
        let raw_only = parse_sse_line(
            r#"data: {"choices":[{"delta":{"reasoning_details":[{"type":"reasoning.text","text":"private raw reasoning"}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            raw_only,
            vec![ModelEvent::TextDelta {
                text: String::new()
            }]
        );
        let legacy_reasoning = parse_sse_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"private raw reasoning"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            legacy_reasoning,
            vec![ModelEvent::TextDelta {
                text: String::new()
            }]
        );
    }
    #[test]
    fn ignores_sse_comments() {
        assert_eq!(parse_sse_line(": keepalive").unwrap(), Vec::new());
    }
    #[test]
    fn parses_done() {
        assert_eq!(
            parse_sse_line("data: [DONE]").unwrap(),
            vec![ModelEvent::Completed {
                finish_reason: None
            }]
        );
    }

    #[test]
    fn strips_provider_channel_tokens_from_tool_names() {
        let events = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"run_command<|channel|>commentary","arguments":"{}"}}]},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert!(matches!(
            events.as_slice(),
            [ModelEvent::ToolCallDelta { name: Some(name), .. }] if name == "run_command"
        ));
        assert_eq!(
            normalize_tool_name("custom<|channel|>value"),
            "custom<|channel|>value"
        );
    }

    #[test]
    fn preserves_text_multiple_tool_calls_and_completion_from_one_frame() {
        let events = parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"working","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\"a"}},{"index":1,"id":"call_2","function":{"name":"read_file","arguments":"{\"path\":\"b"}}]},"finish_reason":"tool_calls"}]}"#,
        )
        .unwrap();

        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], ModelEvent::TextDelta { text } if text == "working"));
        assert!(
            matches!(&events[1], ModelEvent::ToolCallDelta { index: 0, id: Some(id), .. } if id == "call_1")
        );
        assert!(
            matches!(&events[2], ModelEvent::ToolCallDelta { index: 1, id: Some(id), .. } if id == "call_2")
        );
        assert!(
            matches!(&events[3], ModelEvent::Completed { finish_reason: Some(reason) } if reason == "tool_calls")
        );
    }

    #[test]
    fn http_failures_have_structured_fallback_reasons() {
        assert!(matches!(
            status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            GatewayError::RateLimited(_)
        ));
        assert!(matches!(
            status_error(reqwest::StatusCode::GATEWAY_TIMEOUT),
            GatewayError::Timeout(_)
        ));
        assert!(matches!(
            status_error(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            GatewayError::Unavailable(_)
        ));
        assert!(matches!(
            status_error(reqwest::StatusCode::BAD_REQUEST),
            GatewayError::Provider(_)
        ));
        assert!(matches!(
            status_error(reqwest::StatusCode::PAYLOAD_TOO_LARGE),
            GatewayError::ContextOverflow(_)
        ));
    }

    #[tokio::test]
    async fn malformed_stream_without_newlines_is_rejected_at_the_line_limit() {
        let app = Router::new().route(
            "/stream",
            get(|| async {
                let chunk = Bytes::from(vec![b'x'; MAX_SSE_LINE_BYTES + 1]);
                Body::from_stream(futures_util::stream::once(async move {
                    Ok::<_, Infallible>(chunk)
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .unwrap();
        assert!(response.content_length().is_none());
        let error = sse_stream(response, parse_sse_line)
            .next()
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("oversized SSE line"));
    }

    #[tokio::test]
    async fn tiny_delta_storm_is_rejected_at_the_event_count_limit() {
        const FRAME: &[u8] =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":null}]}\n";
        let app = Router::new().route(
            "/stream",
            get(|| async {
                Body::from_stream(futures_util::stream::iter(std::iter::repeat_n(
                    Ok::<_, Infallible>(Bytes::from_static(FRAME)),
                    MAX_SSE_MODEL_EVENTS + 1,
                )))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .unwrap();
        let results = sse_stream(response, parse_sse_line)
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            results.iter().filter(|result| result.is_ok()).count(),
            MAX_SSE_MODEL_EVENTS
        );
        assert!(
            results
                .last()
                .and_then(|result| result.as_ref().err())
                .is_some_and(|error| error.to_string().contains("event count limit"))
        );
    }

    #[tokio::test]
    async fn sse_preserves_utf8_split_across_chunks_and_parses_final_line() {
        let frame =
            "data: {\"choices\":[{\"delta\":{\"content\":\"中文\"},\"finish_reason\":null}]}";
        let frame = frame.as_bytes().to_vec();
        let app = Router::new().route(
            "/stream",
            get(move || {
                let chunks = frame
                    .clone()
                    .into_iter()
                    .map(|byte| Ok::<_, Infallible>(Bytes::from(vec![byte])));
                async move { Body::from_stream(futures_util::stream::iter(chunks)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = reqwest::Client::new()
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .unwrap();
        let events = sse_stream(response, parse_sse_line)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            events,
            vec![ModelEvent::TextDelta {
                text: "中文".into()
            }]
        );
    }

    #[tokio::test]
    async fn openai_http_contract_sends_controls_and_parses_stream() {
        type Captured = Arc<Mutex<Option<(HeaderMap, Value)>>>;
        let captured: Captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route(
                "/chat/completions",
                post(
                    |State(captured): State<Captured>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *captured.lock().unwrap() = Some((headers, body));
                        let frames = [
                            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":\""}}]},"finish_reason":null}]}),
                            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"a.txt\"}"}}]},"finish_reason":null}]}),
                            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                            json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":4}}),
                        ];
                        let mut stream = frames
                            .into_iter()
                            .map(|frame| format!("data: {frame}\n\n"))
                            .collect::<String>();
                        stream.push_str("data: [DONE]\n\n");
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(stream))
                            .unwrap()
                    },
                ),
            )
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let provider = OpenAiCompatible::new(
            format!("http://{address}"),
            "provider-primary",
            Arc::new(FixedCredentials),
        );
        let events = provider
            .stream(ModelRequest {
                model: "enterprise-model-v1".into(),
                temperature: 0.25,
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: Value::String("inspect".into()),
                }],
                tools: vec![ToolDefinition {
                    name: "read_file".into(),
                    description: "Read a workspace file".into(),
                    parameters: json!({"type":"object","required":["path"]}),
                }],
                max_output_tokens: 321,
                routing: None,
            })
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            ModelEvent::Usage {
                input_tokens: 12,
                output_tokens: 4
            }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ModelEvent::ToolCallDelta { .. }))
                .count(),
            2
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ModelEvent::Completed { finish_reason: Some(reason) } if reason == "tool_calls"
        )));
        let usage_index = events
            .iter()
            .position(|event| matches!(event, ModelEvent::Usage { .. }))
            .unwrap();
        let completion_index = events
            .iter()
            .position(|event| matches!(event, ModelEvent::Completed { .. }))
            .unwrap();
        assert!(usage_index < completion_index);

        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["authorization"], "Bearer short-lived-secret");
        assert_eq!(body["model"], "enterprise-model-v1");
        assert_eq!(body["temperature"], 0.25);
        assert_eq!(body["max_tokens"], 321);
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert!(!body.to_string().contains("short-lived-secret"));
        server.abort();
    }

    #[tokio::test]
    async fn openai_loopback_gateway_can_omit_authorization() {
        let app = Router::new().route(
            "/chat/completions",
            post(|headers: HeaderMap| async move {
                assert!(headers.get("authorization").is_none());
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                    ))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider =
            OpenAiCompatible::without_auth(format!("http://{address}"), Arc::new(FixedCredentials));

        let events = provider
            .stream(ModelRequest {
                model: "default".into(),
                temperature: 0.0,
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: Value::String("hello".into()),
                }],
                tools: Vec::new(),
                max_output_tokens: 32,
                routing: None,
            })
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ModelEvent::TextDelta { text } if text == "ok"))
        );
        server.abort();
    }

    #[tokio::test]
    async fn anthropic_http_contract_translates_tools_history_usage_and_stream() {
        type Captured = Arc<Mutex<Option<(HeaderMap, Value)>>>;
        let captured: Captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route(
                "/messages",
                post(
                    |State(captured): State<Captured>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *captured.lock().unwrap() = Some((headers, body));
                        let stream = [
                            json!({"type":"message_start","message":{"usage":{"input_tokens":13,"output_tokens":0}}}),
                            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"checking"}}),
                            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_2","name":"read_file","input":{}}}),
                            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.txt\"}"}}),
                            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
                        ]
                        .into_iter()
                        .map(|frame| format!("event: fixture\ndata: {frame}\n\n"))
                        .collect::<String>();
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(stream))
                            .unwrap()
                    },
                ),
            )
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = AnthropicMessages::new(
            format!("http://{address}"),
            "provider-primary",
            Arc::new(FixedCredentials),
        );
        let events = provider
            .stream(ModelRequest {
                model: "claude-contract".into(),
                temperature: 0.2,
                messages: vec![
                    ModelMessage { role: "system".into(), content: json!("stay bounded") },
                    ModelMessage { role: "user".into(), content: json!("inspect") },
                    ModelMessage { role: "assistant".into(), content: json!({
                        "text":"", "tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"old.txt\"}"}}]
                    }) },
                    ModelMessage { role: "tool".into(), content: json!({
                        "tool_call_id":"call_1", "name":"read_file", "result":{"text":"old"}
                    }) },
                ],
                tools: vec![ToolDefinition { name: "read_file".into(), description: "Read".into(), parameters: json!({"type":"object"}) }],
                max_output_tokens: 222,
                routing: None,
            })
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(events.contains(&ModelEvent::TextDelta {
            text: "checking".into()
        }));
        assert!(events.contains(&ModelEvent::Usage {
            input_tokens: 13,
            output_tokens: 0
        }));
        assert!(events.contains(&ModelEvent::Usage {
            input_tokens: 0,
            output_tokens: 5
        }));
        assert!(events.iter().any(|event| matches!(event, ModelEvent::ToolCallDelta { id: Some(id), name: Some(name), .. } if id == "call_2" && name == "read_file")));
        assert!(events.iter().any(|event| matches!(event, ModelEvent::Completed { finish_reason: Some(reason) } if reason == "tool_use")));
        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["x-api-key"], "short-lived-secret");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        assert_eq!(body["system"], "stay bounded");
        assert_eq!(body["max_tokens"], 222);
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert!(!body.to_string().contains("short-lived-secret"));
        server.abort();
    }

    #[tokio::test]
    async fn gemini_http_contract_translates_tools_history_usage_and_stream() {
        type Captured = Arc<Mutex<Option<(HeaderMap, Value)>>>;
        let captured: Captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route(
                "/models/gemini-contract:streamGenerateContent",
                post(
                    |State(captured): State<Captured>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *captured.lock().unwrap() = Some((headers, body));
                        let frame = json!({
                            "responseId":"response-7",
                            "candidates":[{"content":{"role":"model","parts":[
                                {"text":"checking"},
                                {"functionCall":{"id":"call_2","name":"read_file","args":{"path":"a.txt"}},"thoughtSignature":"current-signature"}
                            ]},"finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":17,"candidatesTokenCount":6}
                        });
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(format!("data: {frame}\n\n")))
                            .unwrap()
                    },
                ),
            )
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = GeminiGenerateContent::new(
            format!("http://{address}"),
            "provider-primary",
            Arc::new(FixedCredentials),
        );
        let events = provider
            .stream(ModelRequest {
                model: "gemini-contract".into(),
                temperature: 0.3,
                messages: vec![
                    ModelMessage { role: "system".into(), content: json!("stay bounded") },
                    ModelMessage { role: "user".into(), content: json!("inspect") },
                    ModelMessage { role: "assistant".into(), content: json!({
                        "text":"", "tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"old.txt\"}"},"provider_metadata":"prior-signature"}]
                    }) },
                    ModelMessage { role: "tool".into(), content: json!({
                        "tool_call_id":"call_1", "name":"read_file", "result":{"text":"old"}
                    }) },
                ],
                tools: vec![ToolDefinition { name: "read_file".into(), description: "Read".into(), parameters: json!({"type":"object"}) }],
                max_output_tokens: 333,
                routing: None,
            })
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(events.contains(&ModelEvent::TextDelta {
            text: "checking".into()
        }));
        assert!(events.contains(&ModelEvent::Usage {
            input_tokens: 17,
            output_tokens: 6
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ModelEvent::ToolCallDelta {
                    index: 1,
                    id: Some(id),
                    name: Some(name),
                    arguments_delta,
                    provider_metadata: Some(Value::String(signature)),
                } if id == "call_2"
                    && name == "read_file"
                    && arguments_delta.contains("a.txt")
                    && signature == "current-signature"
            )
        }));
        assert!(events.iter().any(|event| matches!(event, ModelEvent::Completed { finish_reason: Some(reason) } if reason == "STOP")));
        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["x-goog-api-key"], "short-lived-secret");
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "stay bounded"
        );
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 333);
        assert_eq!(
            body["tools"][0]["functionDeclarations"][0]["name"],
            "read_file"
        );
        assert_eq!(
            body["contents"][1]["parts"][0]["functionCall"]["id"],
            "call_1"
        );
        assert_eq!(
            body["contents"][1]["parts"][0]["thoughtSignature"],
            "prior-signature"
        );
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
        assert!(!body.to_string().contains("short-lived-secret"));
        server.abort();
    }

    #[tokio::test]
    async fn provider_error_does_not_retain_response_body() {
        let app = Router::new().route(
            "/chat/completions",
            post(|| async { (StatusCode::BAD_REQUEST, "secret prompt echoed by provider") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = OpenAiCompatible::new(
            format!("http://{address}"),
            "provider-primary",
            Arc::new(FixedCredentials),
        );
        let error = provider
            .stream(ModelRequest {
                model: "model".into(),
                temperature: 0.0,
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: Value::String("private input".into()),
                }],
                tools: Vec::new(),
                max_output_tokens: 16,
                routing: None,
            })
            .await
            .err()
            .expect("provider must reject the request")
            .to_string();
        assert!(error.contains("400"));
        assert!(!error.contains("secret prompt echoed by provider"));
        assert!(!error.contains("private input"));
        server.abort();
    }

    struct RouterFixtureProvider {
        outcome: &'static str,
        calls: Arc<std::sync::atomic::AtomicUsize>,
        models: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ModelProvider for RouterFixtureProvider {
        async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.models.lock().unwrap().push(request.model);
            match self.outcome {
                "rate_limited" => Err(GatewayError::RateLimited("fixture 429".into())),
                "credential" => Err(GatewayError::Credential("fixture-key".into())),
                "timeout" => Err(GatewayError::Timeout("fixture timeout".into())),
                _ => Ok(Box::pin(futures_util::stream::iter(vec![
                    Ok(ModelEvent::TextDelta {
                        text: "fallback response".into(),
                    }),
                    Ok(ModelEvent::Completed {
                        finish_reason: Some("stop".into()),
                    }),
                ]))),
            }
        }
    }

    fn routed_request(reasons: Vec<FallbackReason>) -> ModelRequest {
        ModelRequest {
            model: "primary".into(),
            temperature: 0.0,
            messages: vec![ModelMessage {
                role: "user".into(),
                content: json!("work"),
            }],
            tools: Vec::new(),
            max_output_tokens: 32,
            routing: Some(ModelRoutingPolicy {
                routing_order: vec!["primary".into(), "fallback".into()],
                fallback_reasons: reasons,
            }),
        }
    }

    #[tokio::test]
    async fn governed_router_falls_back_only_for_an_explicit_reason_and_records_route() {
        let primary_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fallback_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let models = Arc::new(Mutex::new(Vec::new()));
        let router = GovernedModelRouter::new(vec![
            RoutedModelEndpoint {
                id: "primary".into(),
                provider_model: "provider-primary-v1".into(),
                provider: Arc::new(RouterFixtureProvider {
                    outcome: "rate_limited",
                    calls: primary_calls.clone(),
                    models: models.clone(),
                }),
            },
            RoutedModelEndpoint {
                id: "fallback".into(),
                provider_model: "provider-fallback-v2".into(),
                provider: Arc::new(RouterFixtureProvider {
                    outcome: "success",
                    calls: fallback_calls.clone(),
                    models: models.clone(),
                }),
            },
        ])
        .unwrap();
        let events = router
            .stream(routed_request(vec![FallbackReason::RateLimited]))
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            &events[0],
            ModelEvent::RouteFallback {
                from_model_id,
                to_model_id,
                reason: FallbackReason::RateLimited,
            } if from_model_id == "primary" && to_model_id == "fallback"
        ));
        assert!(matches!(
            &events[1],
            ModelEvent::RouteSelected {
                model_id,
                fallback_from: Some(primary),
                reason: Some(FallbackReason::RateLimited),
            } if model_id == "fallback" && primary == "primary"
        ));
        assert_eq!(primary_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            *models.lock().unwrap(),
            ["provider-primary-v1", "provider-fallback-v2"]
        );
    }

    #[tokio::test]
    async fn governed_router_never_falls_back_for_credentials_or_unapproved_reasons() {
        for (outcome, allowed) in [
            ("credential", vec![FallbackReason::ProviderUnavailable]),
            ("timeout", vec![FallbackReason::RateLimited]),
        ] {
            let fallback_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let router = GovernedModelRouter::new(vec![
                RoutedModelEndpoint {
                    id: "primary".into(),
                    provider_model: "primary".into(),
                    provider: Arc::new(RouterFixtureProvider {
                        outcome,
                        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                        models: Arc::new(Mutex::new(Vec::new())),
                    }),
                },
                RoutedModelEndpoint {
                    id: "fallback".into(),
                    provider_model: "fallback".into(),
                    provider: Arc::new(RouterFixtureProvider {
                        outcome: "success",
                        calls: fallback_calls.clone(),
                        models: Arc::new(Mutex::new(Vec::new())),
                    }),
                },
            ])
            .unwrap();
            assert!(router.stream(routed_request(allowed)).await.is_err());
            assert_eq!(fallback_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }
}
