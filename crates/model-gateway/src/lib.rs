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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
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
    /// Newly observed tokens, not a cumulative provider snapshot.
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
    reasoning_effort: Option<String>,
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
            reasoning_effort: None,
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
            reasoning_effort: None,
            client: http_client(),
            base_url: base_url.into().trim_end_matches('/').into(),
            credential_handle: None,
            credentials,
        }
    }

    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    fn request_body(&self, request: &ModelRequest) -> Result<Value, GatewayError> {
        let effort = request.reasoning_effort.as_deref().or_else(|| {
            // Routing has resolved provider_model by this point. GLM-5.3's max
            // default cannot finish a short title/reflection within this cap.
            if request.tools.is_empty()
                && request.max_output_tokens <= 1024
                && request.model.contains("glm-5.3")
            {
                Some("low")
            } else {
                self.reasoning_effort.as_deref()
            }
        });
        let tools: Vec<Value> = request.tools.iter().map(|t| serde_json::json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect();
        let messages = openai_messages(&request.messages)?;
        let mut body = serde_json::json!({"model":request.model,"temperature":request.temperature,"messages":messages,"tools":tools,"max_tokens":request.max_output_tokens,"stream":true,"stream_options":{"include_usage":true}});
        if let Some(effort) = effort {
            if self.base_url.starts_with("https://openrouter.ai/") {
                body["reasoning"] = serde_json::json!({"effort":effort});
            } else {
                body["reasoning_effort"] = serde_json::json!(effort);
            }
        }
        Ok(body)
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatible {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let mut builder = self
            .client
            .post(format!("{}/chat/completions", self.base_url));
        if let Some(handle) = self.credential_handle.as_deref() {
            let key = self.credentials.resolve(handle).await?;
            builder = builder.bearer_auth(key);
        }
        let body = self.request_body(&request)?;
        let response = builder.json(&body).send().await.map_err(request_error)?;
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
            .into_iter()
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
        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let mut parser = AnthropicSseParser::default();
        Ok(sse_stream(response, move |line| parser.parse(line)))
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
            .into_iter()
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
        let response = self
            .client
            .post(format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                self.base_url, request.model
            ))
            .header("x-goog-api-key", key)
            .json(&body)
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        Ok(sse_stream(response, parse_gemini_sse_line))
    }
}

const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;
const MAX_SSE_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SSE_MODEL_EVENTS: usize = 16_384;

fn sse_stream<P>(response: reqwest::Response, parser: P) -> ModelStream
where
    P: FnMut(&str) -> Result<Vec<ModelEvent>, GatewayError> + Send + 'static,
{
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
        (bytes, Vec::new(), VecDeque::new(), 0_usize, 0_usize, parser),
        move |(mut bytes, mut buffer, mut pending, mut received, mut emitted, mut parser)| async move {
            if let Some(event) = pending.pop_front() {
                emitted = emitted.checked_add(1).ok_or_else(|| {
                    GatewayError::Provider("model stream event count overflow".into())
                })?;
                if emitted > MAX_SSE_MODEL_EVENTS {
                    return Err(GatewayError::Provider(
                        "model stream exceeds the event count limit".into(),
                    ));
                }
                return Ok(Some((
                    event,
                    (bytes, buffer, pending, received, emitted, parser),
                )));
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
                        return Ok(Some((
                            event,
                            (bytes, buffer, pending, received, emitted, parser),
                        )));
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
                        return Ok(Some((
                            event,
                            (bytes, buffer, pending, received, emitted, parser),
                        )));
                    }
                }
            }
        },
    );
    Box::pin(events)
}

fn defer_openai_completion(stream: ModelStream) -> ModelStream {
    let events = futures_util::stream::try_unfold(
        (stream, None, false),
        |(mut stream, mut completion, done): (ModelStream, Option<ModelEvent>, bool)| async move {
            if done {
                return Ok(None);
            }
            loop {
                match stream.next().await {
                    Some(Ok(event @ ModelEvent::Completed { .. })) => {
                        let ModelEvent::Completed { ref finish_reason } = event else {
                            unreachable!()
                        };
                        if finish_reason
                            .as_deref()
                            .is_some_and(|reason| reason.eq_ignore_ascii_case("error"))
                        {
                            // Usage from this frame has already been yielded. An error
                            // must never wait for (or be overwritten by) a later stop.
                            return Ok(Some((event, (stream, None, true))));
                        }
                        if finish_reason.is_some() {
                            completion = Some(event);
                        } else {
                            return Ok(Some((
                                completion.take().unwrap_or(event),
                                (stream, None, true),
                            )));
                        }
                    }
                    Some(Ok(event)) => return Ok(Some((event, (stream, completion, false)))),
                    Some(Err(error)) => return Err(error),
                    None => {
                        return Ok(completion.take().map(|event| (event, (stream, None, true))));
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
                    "tool_calls": content.get("tool_calls").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
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

fn usage_count(usage: &Value, field: &str) -> Result<u64, GatewayError> {
    usage.get(field).and_then(Value::as_u64).ok_or_else(|| {
        GatewayError::InvalidResponse(format!("usage.{field} must be a nonnegative integer"))
    })
}

#[derive(Clone, Copy, Default)]
struct AnthropicUsage {
    input: u64,
    cache_creation: u64,
    cache_read: u64,
    output: u64,
}

impl AnthropicUsage {
    fn update(previous: Option<Self>, usage: &Value) -> Result<(Self, ModelEvent), GatewayError> {
        // Optional cache fields are absent/null in uncached and older responses.
        // A delta omitting a field retains its previous cumulative value.
        fn optional(usage: &Value, field: &str, previous: u64) -> Result<u64, GatewayError> {
            match usage.get(field) {
                None | Some(Value::Null) => Ok(previous),
                Some(_) => usage_count(usage, field),
            }
        }
        let prior = previous.unwrap_or_default();
        let next = Self {
            input: if previous.is_none() {
                usage_count(usage, "input_tokens")?
            } else {
                optional(usage, "input_tokens", prior.input)?
            },
            cache_creation: optional(usage, "cache_creation_input_tokens", prior.cache_creation)?,
            cache_read: optional(usage, "cache_read_input_tokens", prior.cache_read)?,
            output: usage_count(usage, "output_tokens")?,
        };
        let delta = |next: u64, prior: u64| {
            next.checked_sub(prior).ok_or_else(|| {
                GatewayError::InvalidResponse("Anthropic cumulative usage decreased".into())
            })
        };
        // Anthropic input_tokens excludes cache reads and writes. Count the
        // top-level cache totals once, not their nested TTL breakdown again.
        let creation_delta = delta(next.cache_creation, prior.cache_creation)?;
        let read_delta = delta(next.cache_read, prior.cache_read)?;
        let input_tokens = delta(next.input, prior.input)?
            .checked_add(creation_delta)
            .and_then(|input| input.checked_add(read_delta))
            .ok_or_else(|| {
                GatewayError::InvalidResponse("Anthropic input usage overflow".into())
            })?;
        next.input
            .checked_add(next.cache_creation)
            .and_then(|input| input.checked_add(next.cache_read))
            .ok_or_else(|| {
                GatewayError::InvalidResponse("Anthropic input usage overflow".into())
            })?;
        let output_tokens = delta(next.output, prior.output)?;
        Ok((
            next,
            ModelEvent::Usage {
                input_tokens,
                output_tokens,
            },
        ))
    }
}

#[derive(Default)]
struct AnthropicSseParser {
    usage: Option<AnthropicUsage>,
    finish_reason: Option<String>,
    finished: bool,
}

impl AnthropicSseParser {
    fn parse(&mut self, line: &str) -> Result<Vec<ModelEvent>, GatewayError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let Some(value) = sse_data(line)? else {
            return Ok(Vec::new());
        };
        let mut events = Vec::new();
        match value.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if self.usage.is_some() {
                    return Err(GatewayError::InvalidResponse(
                        "duplicate Anthropic message_start".into(),
                    ));
                }
                let (usage, event) = AnthropicUsage::update(None, &value["message"]["usage"])?;
                self.usage = Some(usage);
                events.push(event);
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
            Some("content_block_delta") => match value["delta"].get("type").and_then(Value::as_str)
            {
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
                let prior = self.usage.ok_or_else(|| {
                    GatewayError::InvalidResponse(
                        "Anthropic message_delta without message_start".into(),
                    )
                })?;
                let (usage, event) = AnthropicUsage::update(Some(prior), &value["usage"])?;
                self.usage = Some(usage);
                events.push(event);
                if let Some(reason) = value["delta"]
                    .get("stop_reason")
                    .filter(|reason| !reason.is_null())
                {
                    let reason = reason
                        .as_str()
                        .filter(|reason| !reason.is_empty())
                        .ok_or_else(|| {
                            GatewayError::InvalidResponse("invalid Anthropic stop_reason".into())
                        })?;
                    self.finish_reason = Some(reason.into());
                    if reason.eq_ignore_ascii_case("error") {
                        self.finished = true;
                        events.push(ModelEvent::Completed {
                            finish_reason: Some(reason.into()),
                        });
                    }
                }
            }
            Some("message_stop") => {
                // A start usage sample is provisional. Completion requires the
                // final cumulative usage frame as well as the actual end marker.
                let reason = self.finish_reason.take().ok_or_else(|| {
                    GatewayError::InvalidResponse(
                        "Anthropic message_stop without final usage and stop_reason".into(),
                    )
                })?;
                self.finished = true;
                events.push(ModelEvent::Completed {
                    finish_reason: Some(reason),
                });
            }
            Some("error") => {
                return Err(GatewayError::Provider("Anthropic stream error".into()));
            }
            _ => {}
        }
        Ok(events)
    }
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
            input_tokens: usage_count(usage, "promptTokenCount")?,
            output_tokens: usage_count(usage, "candidatesTokenCount")?,
        });
    }
    if value.get("error").is_some_and(|error| !error.is_null()) {
        events.push(ModelEvent::Completed {
            finish_reason: Some("error".into()),
        });
        return Ok(events);
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
            input_tokens: usage_count(usage, "prompt_tokens")?,
            output_tokens: usage_count(usage, "completion_tokens")?,
        });
    }
    // An HTTP 200 body can still terminate with a provider error. Preserve any
    // reported usage first, but never translate that error into a normal DONE.
    if value.get("error").is_some_and(|error| !error.is_null()) {
        events.push(ModelEvent::Completed {
            finish_reason: Some("error".into()),
        });
        return Ok(events);
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

    async fn http_fixture_events(
        anthropic: bool,
        frames: Vec<Value>,
        done: bool,
    ) -> Vec<Result<ModelEvent, GatewayError>> {
        let mut body = frames
            .into_iter()
            .map(|frame| format!("data: {frame}\n\n"))
            .collect::<String>();
        if done {
            body.push_str("data: [DONE]\n\n");
        }
        let app = Router::new().route(
            if anthropic {
                "/messages"
            } else {
                "/chat/completions"
            },
            post(move || {
                let body = body.clone();
                async move {
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider: Box<dyn ModelProvider> = if anthropic {
            Box::new(AnthropicMessages::new(
                format!("http://{address}"),
                "provider-primary",
                Arc::new(FixedCredentials),
            ))
        } else {
            Box::new(OpenAiCompatible::without_auth(
                format!("http://{address}"),
                Arc::new(FixedCredentials),
            ))
        };
        let events = provider
            .stream(ModelRequest {
                reasoning_effort: None,
                model: "fixture".into(),
                temperature: 0.0,
                messages: vec![],
                tools: vec![],
                max_output_tokens: 128,
                routing: None,
            })
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await;
        server.abort();
        events
    }

    #[tokio::test]
    async fn openai_http_error_is_terminal_despite_later_tools_text_and_stop() {
        for error in [
            json!({"error":{"message":"synthetic"},"usage":{"prompt_tokens":12,"completion_tokens":3}}),
            json!({"choices":[{"delta":{},"finish_reason":"ERROR"}],"usage":{"prompt_tokens":12,"completion_tokens":3}}),
        ] {
            let events = http_fixture_events(false, vec![
                error,
                json!({"choices":[{"delta":{"content":"must not be delivered","tool_calls":[{"index":0,"id":"bad","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"stop"}]}),
            ], true).await.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(
                events.len(),
                2,
                "only pre-error usage and terminal error may be delivered: {events:?}"
            );
            assert_eq!(
                events[0],
                ModelEvent::Usage {
                    input_tokens: 12,
                    output_tokens: 3
                }
            );
            assert!(
                matches!(&events[1], ModelEvent::Completed {finish_reason:Some(reason)} if reason.eq_ignore_ascii_case("error"))
            );
        }
    }

    #[tokio::test]
    async fn anthropic_http_normalizes_cumulative_usage_and_cache_inputs_per_stream() {
        for _ in 0..2 {
            let events = http_fixture_events(true, vec![
                json!({"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}}}),
                json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
                json!({"type":"message_delta","delta":{"stop_reason":null},"usage":{"output_tokens":5}}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":30,"cache_creation_input_tokens":110,"cache_read_input_tokens":220,"output_tokens":15}}),
                json!({"type":"message_stop"}),
            ], false).await.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
            let totals = events
                .iter()
                .fold((0, 0), |(input, output), event| match event {
                    ModelEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => (input + input_tokens, output + output_tokens),
                    _ => (input, output),
                });
            assert_eq!(totals, (360, 15));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, ModelEvent::Completed { .. }))
                    .count(),
                1
            );
            assert!(
                matches!(events.last(), Some(ModelEvent::Completed { finish_reason: Some(reason) }) if reason == "end_turn")
            );
        }
    }

    #[tokio::test]
    async fn anthropic_http_rejects_incomplete_or_inconsistent_final_usage() {
        let start = json!({"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1}}});
        let delta = json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}});
        let stop = json!({"type":"message_stop"});
        for (ending, frames, known_output) in [
            ("no_final_delta", vec![start.clone(), stop.clone()], 1),
            ("no_stop", vec![start.clone(), delta.clone()], 15),
            (
                "missing_usage",
                vec![
                    start.clone(),
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
                    stop.clone(),
                ],
                1,
            ),
            (
                "decreasing_output",
                vec![
                    start.clone(),
                    json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":0}}),
                    stop.clone(),
                ],
                1,
            ),
            (
                "duplicate_start",
                vec![start.clone(), start.clone(), delta.clone(), stop.clone()],
                1,
            ),
            (
                "error_after_delta",
                vec![
                    start,
                    delta,
                    json!({"type":"error","error":{"type":"synthetic"}}),
                    stop,
                ],
                15,
            ),
        ] {
            let events = http_fixture_events(true, frames, false).await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(ModelEvent::Completed { .. }))),
                "{ending}"
            );
            if ending != "no_stop" {
                assert!(events.last().unwrap().is_err(), "{ending}");
            }
            let totals = events
                .iter()
                .fold((0, 0), |(input, output), event| match event {
                    Ok(ModelEvent::Usage {
                        input_tokens,
                        output_tokens,
                    }) => (input + input_tokens, output + output_tokens),
                    _ => (input, output),
                });
            assert_eq!(totals, (25, known_output), "{ending}");
        }
    }

    #[test]
    fn anthropic_cache_fields_follow_optional_schema_without_double_counting_details() {
        for cache in [
            json!({}),
            json!({"cache_creation_input_tokens":null,"cache_read_input_tokens":null}),
        ] {
            let mut usage = json!({"input_tokens":25,"output_tokens":1});
            usage
                .as_object_mut()
                .unwrap()
                .extend(cache.as_object().unwrap().clone());
            let (prior, event) = AnthropicUsage::update(None, &usage).unwrap();
            assert_eq!(
                event,
                ModelEvent::Usage {
                    input_tokens: 25,
                    output_tokens: 1
                }
            );
            let (_, event) = AnthropicUsage::update(
                Some(prior),
                &json!({"output_tokens":15,"input_tokens":null}),
            )
            .unwrap();
            assert_eq!(
                event,
                ModelEvent::Usage {
                    input_tokens: 0,
                    output_tokens: 14
                }
            );
        }
        let base = json!({"input_tokens":25,"output_tokens":1,"cache_creation_input_tokens":100,"cache_read_input_tokens":200,"cache_creation":{"ephemeral_5m_input_tokens":60,"ephemeral_1h_input_tokens":40}});
        let (prior, event) = AnthropicUsage::update(None, &base).unwrap();
        assert_eq!(
            event,
            ModelEvent::Usage {
                input_tokens: 325,
                output_tokens: 1
            }
        );
        for field in [
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
            "output_tokens",
        ] {
            for invalid in [json!(-1), json!("0"), json!(false), json!(1.5)] {
                let mut usage = base.clone();
                usage[field] = invalid;
                assert!(AnthropicUsage::update(None, &usage).is_err(), "{field}");
            }
            let mut decreasing = base.clone();
            decreasing[field] = json!(0);
            assert!(
                AnthropicUsage::update(Some(prior), &decreasing).is_err(),
                "{field}"
            );
        }
        let mut overflow = base;
        overflow["input_tokens"] = json!(u64::MAX);
        assert!(AnthropicUsage::update(None, &overflow).is_err());
        for missing in [json!({}), json!({"output_tokens":null})] {
            assert!(AnthropicUsage::update(Some(prior), &missing).is_err());
        }
    }

    #[async_trait]
    impl CredentialProvider for FixedCredentials {
        async fn resolve(&self, handle: &str) -> Result<String, GatewayError> {
            assert_eq!(handle, "provider-primary");
            Ok("short-lived-secret".into())
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

    fn parse_anthropic_sse_line(line: &str) -> Result<Vec<ModelEvent>, GatewayError> {
        AnthropicSseParser::default().parse(line)
    }

    #[test]
    fn missing_or_invalid_usage_fields_are_never_reported_as_known_zero() {
        for (parser, input, output) in [
            (
                parse_sse_line as fn(&str) -> Result<Vec<ModelEvent>, GatewayError>,
                "prompt_tokens",
                "completion_tokens",
            ),
            (parse_anthropic_sse_line, "input_tokens", "output_tokens"),
            (
                parse_gemini_sse_line,
                "promptTokenCount",
                "candidatesTokenCount",
            ),
        ] {
            for invalid in [Value::Null, json!(-1), json!("0"), json!(1.5), json!(false)] {
                for missing_field in [input, output] {
                    let mut usage = json!({input: 0, output: 0});
                    usage[missing_field] = invalid.clone();
                    let value = json!({
                        "choices": [], "usage": usage,
                        "type": "message_start", "message": {"usage":usage},
                        "usageMetadata": usage,
                    });
                    assert!(parser(&format!("data: {value}")).is_err());
                    usage.as_object_mut().unwrap().remove(missing_field);
                    let value = json!({"choices": [], "usage": usage, "type": "message_start", "message": {"usage":usage}, "usageMetadata":usage});
                    assert!(parser(&format!("data: {value}")).is_err());
                }
            }
            let usage = json!({input:0,output:0});
            let valid = json!({"choices":[],"usage":usage,"type":"message_start","message":{"usage":usage},"usageMetadata":usage});
            assert!(
                parser(&format!("data: {valid}"))
                    .unwrap()
                    .contains(&ModelEvent::Usage {
                        input_tokens: 0,
                        output_tokens: 0
                    })
            );
        }
        assert!(
            parse_anthropic_sse_line(r#"data: {"type":"message_start","message":{}}"#).is_err()
        );
        assert!(
            parse_anthropic_sse_line(
                r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#
            )
            .is_err()
        );
        assert!(
            parse_sse_line(r#"data: {"choices":[],"usage":null}"#)
                .unwrap()
                .is_empty()
        );
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

    #[test]
    fn reasoning_controls_preserve_defaults_and_use_provider_wire_formats() {
        let mut request = ModelRequest {
            reasoning_effort: None,
            model: "standard-model".into(),
            temperature: 0.0,
            messages: vec![],
            tools: vec![],
            max_output_tokens: 1024,
            routing: None,
        };
        for base in ["https://example.com/v1", "https://openrouter.ai/api/v1"] {
            let provider = OpenAiCompatible::without_auth(base, Arc::new(FixedCredentials));
            let body = provider.request_body(&request).unwrap();
            assert!(body.get("reasoning").is_none());
            assert!(body.get("reasoning_effort").is_none());
            let provider = provider.with_reasoning_effort(Some("high".into()));
            let control = |body: Value| {
                if base.contains("openrouter.ai") {
                    assert!(body.get("reasoning_effort").is_none());
                    body["reasoning"]["effort"].clone()
                } else {
                    assert!(body.get("reasoning").is_none());
                    body["reasoning_effort"].clone()
                }
            };
            assert_eq!(control(provider.request_body(&request).unwrap()), "high");
            request.reasoning_effort = Some("medium".into());
            assert_eq!(control(provider.request_body(&request).unwrap()), "medium");
            request.reasoning_effort = None;
            // The router replaces a logical alias with this actual provider model.
            request.model = "z-ai/glm-5.3".into();
            assert_eq!(control(provider.request_body(&request).unwrap()), "low");
            request.max_output_tokens = 8192;
            assert_eq!(control(provider.request_body(&request).unwrap()), "high");
            request.max_output_tokens = 1024;
            request.tools.push(ToolDefinition {
                name: "read_file".into(),
                description: "read".into(),
                parameters: json!({}),
            });
            assert_eq!(control(provider.request_body(&request).unwrap()), "high");
            request.tools.clear();
            request.model = "standard-model".into();
        }
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
        )
        .with_reasoning_effort(Some("high".into()));
        let events = provider
            .stream(ModelRequest {
                reasoning_effort: Some("low".into()),
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
        assert_eq!(body["reasoning_effort"], "low");
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
                reasoning_effort: None,
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
                            json!({"type":"message_stop"}),
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
                reasoning_effort: None,
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
                reasoning_effort: None,
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
                reasoning_effort: None,
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
            reasoning_effort: None,
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
