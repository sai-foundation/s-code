//! Bidirectional instant-messaging transports. Authentication and task authorization
//! belong to the caller; the transport only accepts human users in private chats.
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use thiserror::Error;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_TEXT_UTF16: usize = 4000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImError {
    #[error("invalid IM configuration")]
    Configuration,
    #[error("invalid IM message")]
    InvalidMessage,
    #[error("IM service is unreachable")]
    Network,
    #[error("IM credentials were rejected")]
    Unauthorized,
    #[error("IM request was rejected (HTTP {status})")]
    Rejected { status: u16 },
    #[error("IM service response was invalid")]
    InvalidResponse,
    #[error("IM message is missing or cannot be edited")]
    MessageNotFound,
    #[error("IM rate limit reached; retry after {retry_after} seconds")]
    RateLimited { retry_after: u64 },
}

impl ImError {
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited { retry_after } => Some(*retry_after),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImButton {
    pub text: String,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImUpdate {
    pub update_id: i64,
    pub user_id: i64,
    pub chat_id: i64,
    pub kind: ImUpdateKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImUpdateKind {
    Message {
        text: String,
    },
    Callback {
        id: String,
        data: String,
        message_id: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImPoll {
    pub updates: Vec<ImUpdate>,
    /// Includes ignored updates so groups and unsupported events are acknowledged.
    pub next_offset: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImBot {
    pub id: i64,
    pub username: String,
}

#[async_trait]
pub trait ImChannel: Send + Sync {
    async fn poll(&self, offset: i64) -> Result<ImPoll, ImError>;
    async fn send(
        &self,
        chat_id: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<i64, ImError>;
    async fn edit(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<(), ImError>;
    async fn answer_callback(&self, callback_id: &str, text: &str) -> Result<(), ImError>;
}

/// Never exposes its token through Debug or error messages. Production endpoints
/// are fixed: callers cannot redirect a bot credential to a different host.
pub struct TelegramChannel {
    client: Client,
    endpoint: String,
}

impl std::fmt::Debug for TelegramChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramChannel").finish_non_exhaustive()
    }
}

impl TelegramChannel {
    pub fn new(token: String) -> Result<Self, ImError> {
        Self::build(token, "https://api.telegram.org")
    }

    fn build(token: String, base: &str) -> Result<Self, ImError> {
        let (id, secret) = token.split_once(':').ok_or(ImError::Configuration)?;
        if token.len() > 256
            || id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit())
            || secret.is_empty()
            || !secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(ImError::Configuration);
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(35))
            .build()
            .map_err(|_| ImError::Configuration)?;
        Ok(Self {
            client,
            endpoint: format!("{base}/bot{token}"),
        })
    }

    pub async fn get_me(&self) -> Result<ImBot, ImError> {
        let value = self.request("getMe", json!({})).await?;
        let id = positive_id(&value["id"]).ok_or(ImError::InvalidResponse)?;
        let username = value["username"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or(ImError::InvalidResponse)?;
        if value["is_bot"].as_bool() != Some(true) {
            return Err(ImError::InvalidResponse);
        }
        Ok(ImBot {
            id,
            username: username.to_owned(),
        })
    }

    async fn request(&self, method: &str, body: Value) -> Result<Value, ImError> {
        let mut response = self
            .client
            .post(format!("{}/{method}", self.endpoint))
            .timeout(Duration::from_secs(if method == "getUpdates" {
                35
            } else {
                10
            }))
            .json(&body)
            .send()
            .await
            .map_err(|_| ImError::Network)?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(ImError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ImError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(ImError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        // Server descriptions may contain credentials or message contents. They
        // are deliberately never propagated to application logs or chat replies.
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let code = value["error_code"]
            .as_u64()
            .unwrap_or(status.as_u16().into());
        if status == StatusCode::TOO_MANY_REQUESTS || code == 429 {
            return Err(ImError::RateLimited {
                retry_after: value["parameters"]["retry_after"]
                    .as_u64()
                    .unwrap_or(30)
                    .clamp(1, 86400),
            });
        }
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || code == 401
            || code == 403
        {
            return Err(ImError::Unauthorized);
        }
        // Only interpret these narrowly documented edit errors. Descriptions
        // never leave this function, and unrelated 400s remain failures.
        if method == "editMessageText" && status == StatusCode::BAD_REQUEST && code == 400 {
            let description = value["description"].as_str().unwrap_or("");
            if description == "Bad Request: message is not modified"
                || description.starts_with("Bad Request: message is not modified:")
            {
                return Ok(Value::Bool(true));
            }
            if matches!(
                description,
                "Bad Request: message to edit not found" | "Bad Request: message can't be edited"
            ) {
                return Err(ImError::MessageNotFound);
            }
        }
        if !status.is_success() || value["ok"].as_bool() == Some(false) {
            return Err(ImError::Rejected {
                status: u16::try_from(code).unwrap_or(status.as_u16()),
            });
        }
        if value["ok"].as_bool() != Some(true) || value.get("result").is_none() {
            return Err(ImError::InvalidResponse);
        }
        Ok(value["result"].clone())
    }
}

fn positive_id(value: &Value) -> Option<i64> {
    value.as_i64().filter(|n| *n > 0)
}

fn normalize_update(value: &Value, update_id: i64) -> Option<ImUpdate> {
    let (message, from, kind) = if let Some(callback) = value.get("callback_query") {
        let message = callback.get("message")?;
        let id = callback["id"].as_str()?;
        let data = callback["data"].as_str()?;
        if id.is_empty() || id.len() > 256 || data.is_empty() || data.len() > 64 {
            return None;
        }
        (
            message,
            &callback["from"],
            ImUpdateKind::Callback {
                id: id.to_owned(),
                data: data.to_owned(),
                message_id: positive_id(&message["message_id"])?,
            },
        )
    } else {
        let message = value.get("message")?;
        // Forwarding a task does not establish fresh user intent. Likewise, a
        // task queued while the machine was offline must not execute hours later.
        // Callback message dates describe the *original* bot message, so callback
        // expiry is enforced by the application capability rather than here.
        if message.get("forward_origin").is_some()
            || message.get("forward_date").is_some()
            || message.get("forward_from").is_some()
            || message.get("forward_from_chat").is_some()
            || message.get("forward_sender_name").is_some()
            || message["is_automatic_forward"].as_bool() == Some(true)
        {
            return None;
        }
        let date = message["date"].as_i64()?;
        let now = chrono::Utc::now().timestamp();
        if date < now - 600 || date > now + 60 {
            return None;
        }
        let text = message["text"].as_str()?;
        if text.is_empty() || text.len() > 32768 {
            return None;
        }
        (
            message,
            &message["from"],
            ImUpdateKind::Message {
                text: text.to_owned(),
            },
        )
    };
    let user_id = positive_id(&from["id"])?;
    let chat_id = positive_id(&message["chat"]["id"])?;
    if from["is_bot"].as_bool() != Some(false)
        || message["chat"]["type"].as_str() != Some("private")
        || user_id != chat_id
    {
        return None;
    }
    Some(ImUpdate {
        update_id,
        user_id,
        chat_id,
        kind,
    })
}

fn message_body(chat_id: i64, text: &str, buttons: Vec<Vec<ImButton>>) -> Result<Value, ImError> {
    if chat_id <= 0 || text.trim().is_empty() || buttons.len() > 8 {
        return Err(ImError::InvalidMessage);
    }
    let mut rows = Vec::new();
    for row in buttons {
        if row.is_empty() || row.len() > 4 {
            return Err(ImError::InvalidMessage);
        }
        let mut output = Vec::new();
        for button in row {
            if button.text.trim().is_empty()
                || button.text.encode_utf16().count() > 64
                || button.data.is_empty()
                || button.data.len() > 64
            {
                return Err(ImError::InvalidMessage);
            }
            output.push(json!({"text":button.text,"callback_data":button.data}));
        }
        rows.push(output);
    }
    Ok(
        json!({"chat_id":chat_id,"text":truncate_text(text,MAX_TEXT_UTF16),
        "link_preview_options":{"is_disabled":true},
        "reply_markup":{"inline_keyboard":rows}}),
    )
}

/// Telegram counts UTF-16 code units, not UTF-8 bytes. Truncation never splits a
/// Unicode scalar and includes the ellipsis within the requested limit.
pub fn truncate_text(text: &str, max_utf16: usize) -> String {
    let max_utf16 = max_utf16.min(MAX_TEXT_UTF16);
    if text.encode_utf16().count() <= max_utf16 {
        return text.to_owned();
    }
    if max_utf16 == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for c in text.chars() {
        if used + c.len_utf16() > max_utf16 - 1 {
            break;
        }
        used += c.len_utf16();
        result.push(c);
    }
    result.push('…');
    result
}

#[async_trait]
impl ImChannel for TelegramChannel {
    async fn poll(&self, offset: i64) -> Result<ImPoll, ImError> {
        if offset < 0 {
            return Err(ImError::InvalidMessage);
        }
        let value = self
            .request(
                "getUpdates",
                json!({"offset":offset,"timeout":25,"limit":100,
            "allowed_updates":["message","callback_query"]}),
            )
            .await?;
        let raw = value
            .as_array()
            .filter(|v| v.len() <= 100)
            .ok_or(ImError::InvalidResponse)?;
        let mut updates = Vec::new();
        let mut next_offset = offset;
        for item in raw {
            let id = item["update_id"]
                .as_i64()
                .filter(|id| *id >= 0 && *id < i64::MAX)
                .ok_or(ImError::InvalidResponse)?;
            next_offset = next_offset.max(id + 1);
            if id >= offset
                && let Some(update) = normalize_update(item, id)
            {
                updates.push(update);
            }
        }
        updates.sort_by_key(|update| update.update_id);
        updates.dedup_by_key(|update| update.update_id);
        Ok(ImPoll {
            updates,
            next_offset,
        })
    }

    async fn send(
        &self,
        chat_id: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<i64, ImError> {
        let result = self
            .request("sendMessage", message_body(chat_id, text, buttons)?)
            .await?;
        positive_id(&result["message_id"]).ok_or(ImError::InvalidResponse)
    }

    async fn edit(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        buttons: Vec<Vec<ImButton>>,
    ) -> Result<(), ImError> {
        if message_id <= 0 {
            return Err(ImError::InvalidMessage);
        }
        let mut body = message_body(chat_id, text, buttons)?;
        body["message_id"] = json!(message_id);
        self.request("editMessageText", body).await?;
        Ok(())
    }

    async fn answer_callback(&self, callback_id: &str, text: &str) -> Result<(), ImError> {
        if callback_id.is_empty() || callback_id.len() > 256 {
            return Err(ImError::InvalidMessage);
        }
        self.request(
            "answerCallbackQuery",
            json!({"callback_query_id":callback_id,
            "text":truncate_text(text,200)}),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use std::sync::{Arc, Mutex};

    const TOKEN: &str = "123456:private_TEST-token";

    #[derive(Clone)]
    struct MockState {
        status: StatusCode,
        response: String,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    struct MockServer {
        channel: TelegramChannel,
        requests: Arc<Mutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn mock(status: StatusCode, response: Value) -> MockServer {
        mock_raw(status, response.to_string()).await
    }

    async fn mock_raw(status: StatusCode, response: String) -> MockServer {
        async fn handler(
            State(state): State<MockState>,
            Json(body): Json<Value>,
        ) -> impl IntoResponse {
            state.requests.lock().unwrap().push(body);
            let mut headers = HeaderMap::new();
            headers.insert(
                "location",
                "http://127.0.0.1:1/should-not-follow".parse().unwrap(),
            );
            (state.status, headers, state.response)
        }
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = MockState {
            status,
            response,
            requests: requests.clone(),
        };
        let app = Router::new()
            .route("/{*path}", post(handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockServer {
            channel: TelegramChannel::build(TOKEN.to_owned(), &base).unwrap(),
            requests,
            task,
        }
    }

    fn message(id: i64, user: i64, chat: i64, kind: &str, bot: bool) -> Value {
        json!({"update_id":id,"message":{"message_id":1,"date":chrono::Utc::now().timestamp(),"from":{"id":user,"is_bot":bot},
            "chat":{"id":chat,"type":kind},"text":"hello"}})
    }

    #[tokio::test]
    async fn poll_only_accepts_private_humans_but_advances_all_updates() {
        let callback = json!({"update_id":4,"callback_query":{"id":"query1","data":"approve:abc",
            "from":{"id":42,"is_bot":false},"message":{"message_id":99,"chat":{"id":42,"type":"private"}}}});
        let mut forged = callback.clone();
        forged["update_id"] = json!(5);
        forged["callback_query"]["from"]["id"] = json!(43);
        let server = mock(
            StatusCode::OK,
            json!({"ok":true,"result":[
                message(0,42,42,"private",false), message(1,42,42,"private",false),
                message(2,42,-9,"group",false),message(3,42,42,"private",true),callback,forged,
                {"update_id":6,"edited_message":{"text":"unsupported"}},
                {"update_id":7,"message":{"photo":[{}]}}
            ]}),
        )
        .await;
        let result = server.channel.poll(1).await.unwrap();
        assert_eq!(result.next_offset, 8);
        assert_eq!(result.updates.len(), 2);
        assert_eq!(
            result.updates[0].kind,
            ImUpdateKind::Message {
                text: "hello".into()
            }
        );
        assert_eq!(
            result.updates[1],
            ImUpdate {
                update_id: 4,
                user_id: 42,
                chat_id: 42,
                kind: ImUpdateKind::Callback {
                    id: "query1".into(),
                    data: "approve:abc".into(),
                    message_id: 99
                }
            }
        );
        let request = &server.requests.lock().unwrap()[0];
        assert_eq!(request["offset"], 1);
        assert_eq!(request["timeout"], 25);
        assert_eq!(
            request["allowed_updates"],
            json!(["message", "callback_query"])
        );
    }

    #[tokio::test]
    async fn rate_limits_and_credentials_never_echo_server_text() {
        let leaked = format!("https://api.telegram.org/bot{TOKEN}/getUpdates");
        for (status, code, expected) in [
            (
                StatusCode::TOO_MANY_REQUESTS,
                429,
                ImError::RateLimited { retry_after: 17 },
            ),
            (
                StatusCode::OK,
                429,
                ImError::RateLimited { retry_after: 17 },
            ),
            (StatusCode::UNAUTHORIZED, 401, ImError::Unauthorized),
            (
                StatusCode::BAD_REQUEST,
                400,
                ImError::Rejected { status: 400 },
            ),
        ] {
            let server = mock(
                status,
                json!({"ok":false,"error_code":code,"description":leaked,
                "parameters":{"retry_after":17}}),
            )
            .await;
            let error = server.channel.poll(0).await.unwrap_err();
            assert_eq!(error, expected);
            assert!(!error.to_string().contains(TOKEN));
            assert!(!format!("{:?}", server.channel).contains(TOKEN));
        }
    }

    #[tokio::test]
    async fn redirects_and_invalid_or_oversize_bodies_fail_closed() {
        let redirect = mock(StatusCode::FOUND, json!({})).await;
        assert_eq!(
            redirect.channel.poll(0).await.unwrap_err(),
            ImError::Rejected { status: 302 }
        );
        let bad = mock_raw(StatusCode::OK, format!("not JSON: {TOKEN}")).await;
        assert_eq!(
            bad.channel.poll(0).await.unwrap_err(),
            ImError::InvalidResponse
        );
        let huge = mock_raw(StatusCode::OK, " ".repeat(MAX_RESPONSE_BYTES + 1)).await;
        assert_eq!(
            huge.channel.poll(0).await.unwrap_err(),
            ImError::InvalidResponse
        );
        let missing_id = mock(StatusCode::OK, json!({"ok":true,"result":[{"message":{}}]})).await;
        assert_eq!(
            missing_id.channel.poll(0).await.unwrap_err(),
            ImError::InvalidResponse
        );
    }

    #[tokio::test]
    async fn send_edit_and_callback_use_plain_text_and_bounded_buttons() {
        let server = mock(
            StatusCode::OK,
            json!({"ok":true,"result":{"message_id":123}}),
        )
        .await;
        let buttons = vec![vec![ImButton {
            text: "Approve".into(),
            data: "opaque-id".into(),
        }]];
        assert_eq!(
            server
                .channel
                .send(42, "<b>literal</b>", buttons.clone())
                .await
                .unwrap(),
            123
        );
        server
            .channel
            .edit(42, 123, &"🦀".repeat(4000), buttons)
            .await
            .unwrap();
        server
            .channel
            .answer_callback("callback-1", &"🦀".repeat(200))
            .await
            .unwrap();
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests[0]["text"], "<b>literal</b>");
        assert!(requests[0].get("parse_mode").is_none());
        assert_eq!(
            requests[0]["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
            "opaque-id"
        );
        assert!(requests[1]["text"].as_str().unwrap().encode_utf16().count() <= 4000);
        assert_eq!(requests[1]["message_id"], 123);
        assert!(requests[2]["text"].as_str().unwrap().encode_utf16().count() <= 200);
    }

    #[tokio::test]
    async fn get_me_validates_bot_identity_and_local_validation_prevents_requests() {
        let server = mock(
            StatusCode::OK,
            json!({"ok":true,"result":{"id":12,"username":"scode_bot","is_bot":true}}),
        )
        .await;
        assert_eq!(
            server.channel.get_me().await.unwrap(),
            ImBot {
                id: 12,
                username: "scode_bot".into()
            }
        );
        let buttons = vec![vec![ImButton {
            text: "Accept".into(),
            data: "🦀".repeat(17),
        }]];
        assert_eq!(
            server.channel.send(42, "text", buttons).await.unwrap_err(),
            ImError::InvalidMessage
        );
        assert_eq!(
            server.channel.send(-42, "text", vec![]).await.unwrap_err(),
            ImError::InvalidMessage
        );
        assert_eq!(
            server.channel.poll(-1).await.unwrap_err(),
            ImError::InvalidMessage
        );
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        let human = mock(
            StatusCode::OK,
            json!({"ok":true,"result":{"id":12,"username":"human","is_bot":false}}),
        )
        .await;
        assert_eq!(
            human.channel.get_me().await.unwrap_err(),
            ImError::InvalidResponse
        );
    }

    #[tokio::test]
    async fn harmless_edit_retries_and_deleted_messages_have_distinct_results() {
        for description in [
            "Bad Request: message is not modified",
            "Bad Request: message is not modified: specified new message content and reply markup are exactly the same as a current content and reply markup of the message",
        ] {
            let server = mock(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error_code":400,"description":description}),
            )
            .await;
            server.channel.edit(42, 1, "same", vec![]).await.unwrap();
            // Identical server text cannot turn a failed *send* into success.
            assert_eq!(
                server.channel.send(42, "same", vec![]).await.unwrap_err(),
                ImError::Rejected { status: 400 }
            );
        }
        for description in [
            "Bad Request: message to edit not found",
            "Bad Request: message can't be edited",
        ] {
            let server = mock(
                StatusCode::BAD_REQUEST,
                json!({"ok":false,"error_code":400,"description":description}),
            )
            .await;
            assert_eq!(
                server
                    .channel
                    .edit(42, 1, "text", vec![])
                    .await
                    .unwrap_err(),
                ImError::MessageNotFound
            );
        }
        let server = mock(
            StatusCode::BAD_REQUEST,
            json!({"ok":false,"error_code":400,
            "description":"Bad Request: unrelated invalid field"}),
        )
        .await;
        assert_eq!(
            server
                .channel
                .edit(42, 1, "text", vec![])
                .await
                .unwrap_err(),
            ImError::Rejected { status: 400 }
        );
        assert_eq!(
            ImError::RateLimited { retry_after: 99 }.retry_after_secs(),
            Some(99)
        );
        assert_eq!(ImError::Network.retry_after_secs(), None);
    }

    #[tokio::test]
    async fn stale_and_forwarded_tasks_are_ignored_but_acknowledged() {
        let valid = message(1, 42, 42, "private", false);
        let mut raw = vec![valid.clone()];
        for (field, value) in [
            (
                "forward_origin",
                json!({"type":"hidden_user","sender_user_name":"sender"}),
            ),
            ("forward_date", json!(chrono::Utc::now().timestamp())),
            ("forward_from", json!({"id":99})),
            ("forward_from_chat", json!({"id":99})),
            ("forward_sender_name", json!("sender")),
            ("is_automatic_forward", json!(true)),
            ("date", json!(chrono::Utc::now().timestamp() - 601)),
            ("date", json!(chrono::Utc::now().timestamp() + 120)),
            ("date", Value::Null),
        ] {
            let mut rejected = valid.clone();
            rejected["update_id"] = json!(raw.len() + 1);
            rejected["message"][field] = value;
            raw.push(rejected);
        }
        let expected_offset = raw.len() as i64 + 1;
        let server = mock(StatusCode::OK, json!({"ok":true,"result":raw})).await;
        let poll = server.channel.poll(0).await.unwrap();
        assert_eq!(poll.next_offset, expected_offset);
        assert_eq!(poll.updates.len(), 1);
        assert_eq!(poll.updates[0].update_id, 1);
    }

    #[test]
    fn callbacks_require_complete_human_private_chat_identity() {
        let valid = json!({"callback_query":{"id":"callback-1","data":"approve:nonce",
            "from":{"id":42,"is_bot":false},
            "message":{"message_id":1,"chat":{"id":42,"type":"private"}}}});
        assert!(normalize_update(&valid, 1).is_some());
        for (pointer, value) in [
            ("/callback_query/from/is_bot", json!(true)),
            ("/callback_query/from/id", json!(43)),
            ("/callback_query/from/id", Value::Null),
            ("/callback_query/message/chat/type", json!("supergroup")),
            ("/callback_query/message/message_id", json!(0)),
            ("/callback_query/data", json!("x".repeat(65))),
            ("/callback_query/id", json!("")),
            ("/callback_query/message", Value::Null),
        ] {
            let mut rejected = valid.clone();
            *rejected.pointer_mut(pointer).unwrap() = value;
            assert!(normalize_update(&rejected, 1).is_none(), "{pointer}");
        }
    }

    #[test]
    fn token_validation_and_utf16_truncation() {
        for token in [
            "",
            ":secret",
            "123:",
            "x:secret",
            "123:secret/evil",
            "123:secret?x=1",
            "123:secret\n",
        ] {
            assert_eq!(
                TelegramChannel::new(token.into()).unwrap_err(),
                ImError::Configuration
            );
        }
        assert_eq!(truncate_text("hi", 0), "");
        assert_eq!(truncate_text("🦀abc", 1), "…");
        assert_eq!(truncate_text("🦀abc", 2), "…");
        assert_eq!(truncate_text("🦀abc", 3), "🦀…");
        assert_eq!(truncate_text("🦀abc", 5), "🦀abc");
        assert!(
            truncate_text(&"x".repeat(5000), 9999)
                .encode_utf16()
                .count()
                <= 4000
        );
    }
}
