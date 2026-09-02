use async_trait::async_trait;
use hmac::{Hmac, Mac};
use opencoding_identity::TeamGrantVerifier;
use opencoding_protocol::{Id, Scope};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{sync::Arc, time::Duration};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("credential unavailable: {0}")]
    Credential(String),
    #[error("invalid connector configuration: {0}")]
    Configuration(String),
    #[error("connector request failed: {0}")]
    Request(String),
    #[error("connector response was invalid: {0}")]
    Response(String),
    #[error("webhook signature is invalid")]
    InvalidWebhookSignature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCapabilities {
    pub connector_id: String,
    pub version: String,
    pub read_issue: bool,
    pub create_draft_pr: bool,
    pub write_back: bool,
    pub verify_webhook: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionContext {
    pub scope: Scope,
    pub session_id: Id,
    pub task_id: Id,
    pub actor_reason: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRequestEvidence {
    pub summary: String,
    pub risk: String,
    pub tests: Vec<String>,
    pub model: String,
    pub approved_diff_hash: String,
    pub audit_event_ids: Vec<Id>,
    #[serde(default)]
    pub reviewer_suggestions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateDraftPullRequest {
    pub context: ActionContext,
    pub title: String,
    pub base: String,
    pub head: String,
    pub expected_head_oid: String,
    pub evidence: PullRequestEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub external_id: String,
    pub url: String,
    pub number: u64,
    pub draft: bool,
    pub head_oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIssue {
    pub external_id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteBack {
    pub context: ActionContext,
    pub issue_number: u64,
    pub expected_issue_version: String,
    pub message: String,
    pub evidence_urls: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkManagementCapabilities {
    pub connector_id: String,
    pub version: String,
    pub read_task: bool,
    pub write_back: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatCapabilities {
    pub connector_id: String,
    pub version: String,
    pub send_notification: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TeamNotification {
    pub context: ActionContext,
    pub channel: String,
    pub message: String,
    pub evidence_urls: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiCapabilities {
    pub connector_id: String,
    pub version: String,
    pub read_commit_checks: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitChecksRequest {
    pub context: ActionContext,
    pub commit_oid: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitCheck {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEventCapabilities {
    pub connector_id: String,
    pub version: String,
    pub export_event: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SecurityEvent {
    pub context: ActionContext,
    pub event_type: String,
    pub severity: String,
    pub resource_type: String,
    pub resource_id: Id,
    pub evidence_ids: Vec<Id>,
    pub occurred_at: String,
}

#[async_trait]
pub trait CredentialBroker: Send + Sync {
    async fn issue(&self, handle: &str, context: &ActionContext) -> Result<String, ConnectorError>;
}

pub struct EnvironmentCredentialBroker;

#[async_trait]
impl CredentialBroker for EnvironmentCredentialBroker {
    async fn issue(&self, handle: &str, _: &ActionContext) -> Result<String, ConnectorError> {
        std::env::var(handle).map_err(|_| ConnectorError::Credential(handle.into()))
    }
}

#[async_trait]
pub trait SourceControlConnector: Send + Sync {
    fn capabilities(&self) -> ConnectorCapabilities;
    async fn read_issue(
        &self,
        number: u64,
        context: &ActionContext,
    ) -> Result<ExternalIssue, ConnectorError>;
    async fn create_draft_pr(
        &self,
        request: CreateDraftPullRequest,
    ) -> Result<PullRequest, ConnectorError>;
    async fn write_back(&self, request: WriteBack) -> Result<(), ConnectorError>;
}

#[async_trait]
pub trait WorkManagementConnector: Send + Sync {
    fn capabilities(&self) -> WorkManagementCapabilities;
    async fn read_task(
        &self,
        number: u64,
        context: &ActionContext,
    ) -> Result<ExternalIssue, ConnectorError>;
    async fn write_back(&self, request: WriteBack) -> Result<(), ConnectorError>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceManagementCapabilities {
    pub connector_id: String,
    pub version: String,
    pub read_record: bool,
    pub append_work_note: bool,
    pub signed_four_eyes_approval: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceNowRecord {
    pub sys_id: String,
    pub number: String,
    pub short_description: String,
    pub description: String,
    pub state: String,
    pub sys_updated_on: String,
    pub sys_mod_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceNowReadRequest {
    pub context: ActionContext,
    pub record_sys_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceNowWorkNote {
    pub context: ActionContext,
    pub record_sys_id: String,
    pub expected_mod_count: u64,
    pub work_note: String,
    pub evidence_urls: Vec<String>,
    pub approval_grant: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceNowWriteResult {
    pub sys_id: String,
    pub sys_mod_count: u64,
    pub replayed: bool,
}

#[async_trait]
pub trait ServiceManagementConnector: Send + Sync {
    fn capabilities(&self) -> ServiceManagementCapabilities;
    async fn read_record(
        &self,
        request: ServiceNowReadRequest,
    ) -> Result<ServiceNowRecord, ConnectorError>;
    async fn append_work_note(
        &self,
        request: ServiceNowWorkNote,
    ) -> Result<ServiceNowWriteResult, ConnectorError>;
}

#[async_trait]
pub trait ChatConnector: Send + Sync {
    fn capabilities(&self) -> ChatCapabilities;
    async fn send_notification(&self, request: TeamNotification) -> Result<(), ConnectorError>;
}

#[async_trait]
pub trait ContinuousIntegrationConnector: Send + Sync {
    fn capabilities(&self) -> CiCapabilities;
    async fn read_commit_checks(
        &self,
        request: CommitChecksRequest,
    ) -> Result<Vec<CommitCheck>, ConnectorError>;
}

#[async_trait]
pub trait SecurityEventConnector: Send + Sync {
    fn capabilities(&self) -> SecurityEventCapabilities;
    async fn export_event(&self, request: SecurityEvent) -> Result<(), ConnectorError>;
}

pub struct JiraWorkManagementConnector {
    client: reqwest::Client,
    api_base: String,
    project_key: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl JiraWorkManagementConnector {
    pub fn new(
        api_base: impl Into<String>,
        project_key: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        let api_base = validated_api_base(api_base.into())?;
        let project_key = project_key.into();
        if project_key.is_empty()
            || !project_key
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(ConnectorError::Configuration(
                "Jira project key must contain uppercase ASCII letters, digits, or underscores"
                    .into(),
            ));
        }
        Ok(Self {
            client: connector_http_client()?,
            api_base,
            project_key,
            credential_handle: credential_handle.into(),
            credentials,
        })
    }

    fn issue_key(&self, number: u64) -> String {
        format!("{}-{number}", self.project_key)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        context: &ActionContext,
        expected_version: Option<&str>,
        body: Option<Value>,
    ) -> Result<reqwest::Response, ConnectorError> {
        validate_action_context(context)?;
        let token = self
            .credentials
            .issue(&self.credential_handle, context)
            .await?;
        let mut request = self
            .client
            .request(method, format!("{}{}", self.api_base, path))
            .bearer_auth(token)
            .header("accept", "application/json")
            .header("x-opencoding-idempotency-key", &context.idempotency_key)
            .header("x-opencoding-team", &context.scope.team_id.0)
            .header("x-opencoding-task", &context.task_id.0)
            .header("x-opencoding-session", &context.session_id.0);
        if let Some(version) = expected_version {
            request = request.header("if-match", version);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))
    }
}

#[async_trait]
impl WorkManagementConnector for JiraWorkManagementConnector {
    fn capabilities(&self) -> WorkManagementCapabilities {
        WorkManagementCapabilities {
            connector_id: "jira-cloud".into(),
            version: "1".into(),
            read_task: true,
            write_back: true,
        }
    }

    async fn read_task(
        &self,
        number: u64,
        context: &ActionContext,
    ) -> Result<ExternalIssue, ConnectorError> {
        let response = self
            .request(
                Method::GET,
                &format!("/rest/api/3/issue/{}", self.issue_key(number)),
                context,
                None,
                None,
            )
            .await?;
        let version = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let value = GitHubEnterpriseConnector::response_json(response).await?;
        let description = value["fields"]["description"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| value["fields"]["description"].to_string());
        Ok(ExternalIssue {
            external_id: value["id"].as_str().unwrap_or_default().into(),
            number,
            title: value["fields"]["summary"]
                .as_str()
                .unwrap_or_default()
                .into(),
            body: description,
            state: value["fields"]["status"]["name"]
                .as_str()
                .unwrap_or_default()
                .into(),
            version,
        })
    }

    async fn write_back(&self, request: WriteBack) -> Result<(), ConnectorError> {
        if request.expected_issue_version.is_empty() || request.message.trim().is_empty() {
            return Err(ConnectorError::Configuration(
                "expected issue version and message are required".into(),
            ));
        }
        let mut paragraphs = vec![json!({
            "type": "paragraph",
            "content": [{"type":"text", "text":request.message}]
        })];
        for evidence in &request.evidence_urls {
            paragraphs.push(json!({
                "type": "paragraph",
                "content": [{"type":"text", "text":format!("Evidence: {evidence}")}]
            }));
        }
        paragraphs.push(json!({
            "type": "paragraph",
            "content": [{"type":"text", "text":format!("opencoding:{}", request.context.idempotency_key)}]
        }));
        let response = self
            .request(
                Method::POST,
                &format!(
                    "/rest/api/3/issue/{}/comment",
                    self.issue_key(request.issue_number)
                ),
                &request.context,
                Some(&request.expected_issue_version),
                Some(json!({"body":{"type":"doc", "version":1, "content":paragraphs}})),
            )
            .await?;
        if !response.status().is_success() {
            return Err(ConnectorError::Request(response.status().to_string()));
        }
        Ok(())
    }
}

pub struct ServiceNowConnector {
    client: reqwest::Client,
    api_base: String,
    table: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
    approval_verifier: TeamGrantVerifier,
}

impl ServiceNowConnector {
    pub fn new(
        api_base: impl Into<String>,
        table: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
        approval_verifier: TeamGrantVerifier,
    ) -> Result<Self, ConnectorError> {
        let table = table.into();
        let standard_table = matches!(
            table.as_str(),
            "incident" | "change_request" | "problem" | "sc_request" | "sc_task"
        );
        let scoped_table = table.starts_with("x_")
            && table.len() <= 128
            && table
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        let credential_handle = credential_handle.into();
        if (!standard_table && !scoped_table)
            || credential_handle.trim().is_empty()
            || credential_handle.len() > 256
        {
            return Err(ConnectorError::Configuration(
                "ServiceNow requires an approved task table and credential handle".into(),
            ));
        }
        Ok(Self {
            client: connector_http_client()?,
            api_base: validated_api_base(api_base.into())?,
            table,
            credential_handle,
            credentials,
            approval_verifier,
        })
    }

    fn record_path(&self, sys_id: &str) -> String {
        format!("/api/now/v1/table/{}/{}", self.table, sys_id)
    }

    async fn token(&self, context: &ActionContext) -> Result<String, ConnectorError> {
        self.credentials
            .issue(&self.credential_handle, context)
            .await
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        context: &ActionContext,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{}", self.api_base, path))
            .bearer_auth(token)
            .header("accept", "application/json")
            .header("x-opencoding-idempotency-key", &context.idempotency_key)
            .header(
                "x-opencoding-organization",
                &context.scope.organization_id.0,
            )
            .header("x-opencoding-team", &context.scope.team_id.0)
            .header("x-opencoding-task", &context.task_id.0)
            .header("x-opencoding-session", &context.session_id.0)
    }

    async fn bounded_response_json(response: reqwest::Response) -> Result<Value, ConnectorError> {
        let status = response.status();
        if !status.is_success() {
            return Err(ConnectorError::Request(status.to_string()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > 256 * 1024)
        {
            return Err(ConnectorError::Response(
                "ServiceNow response exceeds 256 KiB".into(),
            ));
        }
        let body = bounded_response_body(response, 256 * 1024, "ServiceNow").await?;
        serde_json::from_slice(&body).map_err(|error| ConnectorError::Response(error.to_string()))
    }

    fn parse_record(value: &Value) -> Result<ServiceNowRecord, ConnectorError> {
        let value = value
            .get("result")
            .and_then(Value::as_object)
            .ok_or_else(|| ConnectorError::Response("ServiceNow result is missing".into()))?;
        let text = |field: &str, limit: usize| {
            value
                .get(field)
                .and_then(Value::as_str)
                .map(|value| truncate(value, limit))
                .unwrap_or_default()
        };
        let sys_id = text("sys_id", 32);
        validate_servicenow_sys_id(&sys_id)?;
        let sys_mod_count = value
            .get("sys_mod_count")
            .and_then(|value| value.as_str().or_else(|| value.as_u64().map(|_| "")))
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| value.get("sys_mod_count").and_then(Value::as_u64))
            .ok_or_else(|| {
                ConnectorError::Response("ServiceNow sys_mod_count is invalid".into())
            })?;
        Ok(ServiceNowRecord {
            sys_id,
            number: text("number", 128),
            short_description: text("short_description", 4096),
            description: text("description", 32 * 1024),
            state: text("state", 128),
            sys_updated_on: text("sys_updated_on", 64),
            sys_mod_count,
        })
    }

    async fn already_written(
        &self,
        token: &str,
        context: &ActionContext,
        sys_id: &str,
    ) -> Result<bool, ConnectorError> {
        let marker = format!("opencoding:{}", context.idempotency_key);
        let query = format!("element_id={sys_id}^element=work_notes^valueLIKE{}", marker);
        let path = format!(
            "/api/now/v1/table/sys_journal_field?sysparm_query={}&sysparm_fields=sys_id&sysparm_limit=1&sysparm_exclude_reference_link=true",
            url_encode(&query)
        );
        let value = Self::bounded_response_json(
            self.request(Method::GET, &path, token, context)
                .send()
                .await
                .map_err(|error| ConnectorError::Request(error.to_string()))?,
        )
        .await?;
        let records = value
            .get("result")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ConnectorError::Response("ServiceNow journal result is invalid".into())
            })?;
        Ok(!records.is_empty())
    }
}

#[async_trait]
impl ServiceManagementConnector for ServiceNowConnector {
    fn capabilities(&self) -> ServiceManagementCapabilities {
        ServiceManagementCapabilities {
            connector_id: "servicenow-table-api".into(),
            version: "1".into(),
            read_record: true,
            append_work_note: true,
            signed_four_eyes_approval: true,
        }
    }

    async fn read_record(
        &self,
        request: ServiceNowReadRequest,
    ) -> Result<ServiceNowRecord, ConnectorError> {
        validate_action_context(&request.context)?;
        validate_servicenow_sys_id(&request.record_sys_id)?;
        let token = self.token(&request.context).await?;
        let path = format!(
            "{}?sysparm_fields=sys_id,number,short_description,description,state,sys_updated_on,sys_mod_count&sysparm_exclude_reference_link=true&sysparm_display_value=false",
            self.record_path(&request.record_sys_id)
        );
        let response = self
            .request(Method::GET, &path, &token, &request.context)
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        Self::parse_record(&Self::bounded_response_json(response).await?)
    }

    async fn append_work_note(
        &self,
        request: ServiceNowWorkNote,
    ) -> Result<ServiceNowWriteResult, ConnectorError> {
        validate_action_context(&request.context)?;
        validate_servicenow_sys_id(&request.record_sys_id)?;
        validate_connector_text(
            &request.work_note,
            &request.evidence_urls,
            "ServiceNow work note",
            8_000,
        )?;
        let approval = self
            .approval_verifier
            .verify_connector_approval(&request.approval_grant, chrono::Utc::now())
            .map_err(|_| {
                ConnectorError::Configuration("ServiceNow approval grant is invalid".into())
            })?;
        if approval.organization_id != request.context.scope.organization_id
            || approval.team_id != request.context.scope.team_id
            || approval.requested_by != request.context.scope.actor_id
            || approval.resource_id.0 != request.record_sys_id
            || approval.action != "servicenow.append_work_note"
            || approval.idempotency_key != request.context.idempotency_key
        {
            return Err(ConnectorError::Configuration(
                "ServiceNow approval grant does not match the exact write".into(),
            ));
        }
        let token = self.token(&request.context).await?;
        if self
            .already_written(&token, &request.context, &request.record_sys_id)
            .await?
        {
            return Ok(ServiceNowWriteResult {
                sys_id: request.record_sys_id,
                sys_mod_count: request.expected_mod_count,
                replayed: true,
            });
        }
        let current = self
            .read_record(ServiceNowReadRequest {
                context: request.context.clone(),
                record_sys_id: request.record_sys_id.clone(),
            })
            .await?;
        if current.sys_mod_count != request.expected_mod_count {
            return Err(ConnectorError::Request(
                "ServiceNow record changed concurrently".into(),
            ));
        }
        let mut note = request.work_note;
        if !request.evidence_urls.is_empty() {
            note.push_str("\n\nEvidence:\n");
            note.push_str(
                &request
                    .evidence_urls
                    .iter()
                    .map(|url| format!("- {url}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        note.push_str(&format!(
            "\n\nopencoding:{} approval:{}",
            request.context.idempotency_key, approval.approval_id.0
        ));
        let response = self
            .request(
                Method::PATCH,
                &format!(
                    "{}?sysparm_fields=sys_id,number,short_description,description,state,sys_updated_on,sys_mod_count&sysparm_exclude_reference_link=true&sysparm_display_value=false",
                    self.record_path(&request.record_sys_id)
                ),
                &token,
                &request.context,
            )
            .header("content-type", "application/json")
            .header("x-opencoding-approval-id", &approval.approval_id.0)
            .json(&json!({"work_notes":note}))
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        let updated = Self::parse_record(&Self::bounded_response_json(response).await?)?;
        if updated.sys_id != request.record_sys_id
            || updated.sys_mod_count <= request.expected_mod_count
        {
            return Err(ConnectorError::Response(
                "ServiceNow did not confirm the approved record update".into(),
            ));
        }
        Ok(ServiceNowWriteResult {
            sys_id: updated.sys_id,
            sys_mod_count: updated.sys_mod_count,
            replayed: false,
        })
    }
}

pub struct SlackConnector {
    client: reqwest::Client,
    api_base: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl SlackConnector {
    pub fn new(
        api_base: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        Ok(Self {
            client: connector_http_client()?,
            api_base: validated_api_base(api_base.into())?,
            credential_handle: credential_handle.into(),
            credentials,
        })
    }
}

#[async_trait]
impl ChatConnector for SlackConnector {
    fn capabilities(&self) -> ChatCapabilities {
        ChatCapabilities {
            connector_id: "slack".into(),
            version: "1".into(),
            send_notification: true,
        }
    }

    async fn send_notification(&self, request: TeamNotification) -> Result<(), ConnectorError> {
        validate_notification(&request)?;
        let token = self
            .credentials
            .issue(&self.credential_handle, &request.context)
            .await?;
        let text = notification_text(&request);
        let response = self
            .client
            .post(format!("{}/api/chat.postMessage", self.api_base))
            .bearer_auth(token)
            .header(
                "x-opencoding-idempotency-key",
                &request.context.idempotency_key,
            )
            .header("x-opencoding-team", &request.context.scope.team_id.0)
            .json(&json!({"channel":request.channel,"text":text}))
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        let value = response_json(response).await?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(ConnectorError::Response(
                "Slack did not acknowledge the notification".into(),
            ));
        }
        Ok(())
    }
}

pub struct TeamsConnector {
    client: reqwest::Client,
    webhook_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl TeamsConnector {
    pub fn new(
        webhook_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        let webhook_handle = webhook_handle.into();
        if webhook_handle.trim().is_empty() || webhook_handle.len() > 256 {
            return Err(ConnectorError::Configuration(
                "Teams webhook credential handle is required".into(),
            ));
        }
        Ok(Self {
            client: connector_http_client()?,
            webhook_handle,
            credentials,
        })
    }
}

#[async_trait]
impl ChatConnector for TeamsConnector {
    fn capabilities(&self) -> ChatCapabilities {
        ChatCapabilities {
            connector_id: "microsoft-teams".into(),
            version: "1".into(),
            send_notification: true,
        }
    }

    async fn send_notification(&self, request: TeamNotification) -> Result<(), ConnectorError> {
        validate_notification(&request)?;
        let webhook = self
            .credentials
            .issue(&self.webhook_handle, &request.context)
            .await?;
        let webhook = validated_webhook_url(&webhook)?;
        let response = self
            .client
            .post(webhook)
            .header("x-opencoding-idempotency-key", &request.context.idempotency_key)
            .header("x-opencoding-team", &request.context.scope.team_id.0)
            .json(&json!({
                "type":"message",
                "attachments":[{"contentType":"application/vnd.microsoft.card.adaptive","content":{"type":"AdaptiveCard","version":"1.4","body":[{"type":"TextBlock","text":notification_text(&request),"wrap":true}]}}]
            }))
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        ensure_success(response).await
    }
}

pub struct GitHubActionsConnector {
    client: reqwest::Client,
    api_base: String,
    repository: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl GitHubActionsConnector {
    pub fn new(
        api_base: impl Into<String>,
        repository: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        let repository = repository.into();
        validate_repository_slug(&repository)?;
        Ok(Self {
            client: connector_http_client()?,
            api_base: validated_api_base(api_base.into())?,
            repository,
            credential_handle: credential_handle.into(),
            credentials,
        })
    }
}

#[async_trait]
impl ContinuousIntegrationConnector for GitHubActionsConnector {
    fn capabilities(&self) -> CiCapabilities {
        CiCapabilities {
            connector_id: "github-actions".into(),
            version: "1".into(),
            read_commit_checks: true,
        }
    }

    async fn read_commit_checks(
        &self,
        request: CommitChecksRequest,
    ) -> Result<Vec<CommitCheck>, ConnectorError> {
        validate_action_context(&request.context)?;
        if request.commit_oid.len() < 7
            || request.commit_oid.len() > 64
            || !request
                .commit_oid
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ConnectorError::Configuration(
                "CI commit OID must be a bounded hexadecimal identifier".into(),
            ));
        }
        let token = self
            .credentials
            .issue(&self.credential_handle, &request.context)
            .await?;
        let response = self
            .client
            .get(format!(
                "{}/repos/{}/commits/{}/check-runs",
                self.api_base, self.repository, request.commit_oid
            ))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-opencoding-team", &request.context.scope.team_id.0)
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        let value = response_json(response).await?;
        let checks = value["check_runs"].as_array().ok_or_else(|| {
            ConnectorError::Response("GitHub check-runs response is missing check_runs".into())
        })?;
        if checks.len() > 1024 {
            return Err(ConnectorError::Response(
                "CI returned too many check runs".into(),
            ));
        }
        checks
            .iter()
            .map(|check| {
                Ok(CommitCheck {
                    name: truncate(check["name"].as_str().unwrap_or_default(), 256),
                    status: truncate(check["status"].as_str().unwrap_or_default(), 64),
                    conclusion: check["conclusion"]
                        .as_str()
                        .map(|value| truncate(value, 64)),
                    details_url: check["details_url"]
                        .as_str()
                        .map(|value| truncate(value, 2048)),
                })
            })
            .collect()
    }
}

pub struct SplunkHecConnector {
    client: reqwest::Client,
    api_base: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl SplunkHecConnector {
    pub fn new(
        api_base: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        Ok(Self {
            client: connector_http_client()?,
            api_base: validated_api_base(api_base.into())?,
            credential_handle: credential_handle.into(),
            credentials,
        })
    }
}

#[async_trait]
impl SecurityEventConnector for SplunkHecConnector {
    fn capabilities(&self) -> SecurityEventCapabilities {
        SecurityEventCapabilities {
            connector_id: "splunk-hec".into(),
            version: "1".into(),
            export_event: true,
        }
    }

    async fn export_event(&self, request: SecurityEvent) -> Result<(), ConnectorError> {
        validate_action_context(&request.context)?;
        if !bounded_identifier(&request.event_type, 128)
            || !matches!(
                request.severity.as_str(),
                "informational" | "low" | "medium" | "high" | "critical"
            )
            || !bounded_identifier(&request.resource_type, 128)
            || !bounded_identifier(&request.resource_id.0, 256)
            || request.evidence_ids.len() > 128
            || request.occurred_at.len() > 64
        {
            return Err(ConnectorError::Configuration(
                "SIEM event violates the bounded content-free schema".into(),
            ));
        }
        let token = self
            .credentials
            .issue(&self.credential_handle, &request.context)
            .await?;
        let response = self
            .client
            .post(format!("{}/services/collector/event", self.api_base))
            .header("authorization", format!("Splunk {token}"))
            .header(
                "x-opencoding-idempotency-key",
                &request.context.idempotency_key,
            )
            .header("x-opencoding-team", &request.context.scope.team_id.0)
            .json(&json!({"event":{
                "schema_version":1,
                "organization_id":request.context.scope.organization_id,
                "team_id":request.context.scope.team_id,
                "task_id":request.context.task_id,
                "session_id":request.context.session_id,
                "event_type":request.event_type,
                "severity":request.severity,
                "resource_type":request.resource_type,
                "resource_id":request.resource_id,
                "evidence_ids":request.evidence_ids,
                "occurred_at":request.occurred_at,
                "code_content_present":false
            }}))
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))?;
        ensure_success(response).await
    }
}

pub struct GitHubEnterpriseConnector {
    client: reqwest::Client,
    api_base: String,
    repository: String,
    repository_owner: String,
    credential_handle: String,
    credentials: Arc<dyn CredentialBroker>,
}

impl GitHubEnterpriseConnector {
    pub fn new(
        api_base: impl Into<String>,
        repository: impl Into<String>,
        credential_handle: impl Into<String>,
        credentials: Arc<dyn CredentialBroker>,
    ) -> Result<Self, ConnectorError> {
        let api_base = validated_api_base(api_base.into())?;
        let repository = repository.into();
        let (owner, name) = repository
            .split_once('/')
            .ok_or_else(|| ConnectorError::Configuration("repository must be owner/name".into()))?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return Err(ConnectorError::Configuration(
                "repository must be owner/name".into(),
            ));
        }
        Ok(Self {
            client: connector_http_client()?,
            api_base,
            repository_owner: owner.into(),
            repository,
            credential_handle: credential_handle.into(),
            credentials,
        })
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        context: &ActionContext,
        body: Option<Value>,
    ) -> Result<reqwest::Response, ConnectorError> {
        validate_action_context(context)?;
        let token = self
            .credentials
            .issue(&self.credential_handle, context)
            .await?;
        let mut request = self
            .client
            .request(method, format!("{}{}", self.api_base, path))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("x-opencoding-idempotency-key", &context.idempotency_key)
            .header("x-opencoding-team", &context.scope.team_id.0)
            .header("x-opencoding-task", &context.task_id.0)
            .header("x-opencoding-session", &context.session_id.0);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request
            .send()
            .await
            .map_err(|error| ConnectorError::Request(error.to_string()))
    }

    async fn response_json(response: reqwest::Response) -> Result<Value, ConnectorError> {
        response_json(response).await
    }
}

#[async_trait]
impl SourceControlConnector for GitHubEnterpriseConnector {
    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            connector_id: "github-enterprise".into(),
            version: "1".into(),
            read_issue: true,
            create_draft_pr: true,
            write_back: true,
            verify_webhook: true,
        }
    }

    async fn read_issue(
        &self,
        number: u64,
        context: &ActionContext,
    ) -> Result<ExternalIssue, ConnectorError> {
        let response = self
            .request(
                Method::GET,
                &format!("/repos/{}/issues/{number}", self.repository),
                context,
                None,
            )
            .await?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let value = Self::response_json(response).await?;
        Ok(ExternalIssue {
            external_id: value["node_id"].as_str().unwrap_or_default().into(),
            number: value["number"].as_u64().unwrap_or(number),
            title: value["title"].as_str().unwrap_or_default().into(),
            body: value["body"].as_str().unwrap_or_default().into(),
            state: value["state"].as_str().unwrap_or_default().into(),
            version: etag,
        })
    }

    async fn create_draft_pr(
        &self,
        request: CreateDraftPullRequest,
    ) -> Result<PullRequest, ConnectorError> {
        validate_branch(&request.base)?;
        validate_branch(&request.head)?;
        if request.title.trim().is_empty()
            || request.expected_head_oid.len() < 7
            || request.evidence.approved_diff_hash.len() != 64
            || !request
                .evidence
                .approved_diff_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || request.evidence.reviewer_suggestions.len() > 64
            || request
                .evidence
                .reviewer_suggestions
                .iter()
                .any(|reviewer| {
                    reviewer.len() < 2
                        || reviewer.len() > 128
                        || !reviewer.starts_with('@')
                        || !reviewer[1..].chars().all(|character| {
                            character.is_ascii_alphanumeric() || "-_/.".contains(character)
                        })
                })
        {
            return Err(ConnectorError::Configuration(
                "title, expected head OID and approved SHA-256 diff hash are required".into(),
            ));
        }
        let head_query = format!("{}:{}", self.repository_owner, request.head);
        let lookup = self
            .request(
                Method::GET,
                &format!(
                    "/repos/{}/pulls?state=open&head={}",
                    self.repository,
                    url_encode(&head_query)
                ),
                &request.context,
                None,
            )
            .await?;
        let existing = Self::response_json(lookup).await?;
        let body = pull_request_body(&request);
        let value = if let Some(pr) = existing.as_array().and_then(|items| items.first()) {
            let number = pr["number"]
                .as_u64()
                .ok_or_else(|| ConnectorError::Response("existing PR has no number".into()))?;
            let response = self
                .request(
                    Method::PATCH,
                    &format!("/repos/{}/pulls/{number}", self.repository),
                    &request.context,
                    Some(json!({"title": request.title, "body": body, "state": "open"})),
                )
                .await?;
            Self::response_json(response).await?
        } else {
            let response = self
                .request(
                    Method::POST,
                    &format!("/repos/{}/pulls", self.repository),
                    &request.context,
                    Some(json!({
                        "title": request.title,
                        "body": body,
                        "base": request.base,
                        "head": request.head,
                        "draft": true,
                    })),
                )
                .await?;
            Self::response_json(response).await?
        };
        let head_oid = value["head"]["sha"].as_str().unwrap_or_default().to_owned();
        if head_oid != request.expected_head_oid {
            return Err(ConnectorError::Response(
                "SCM head OID does not match the approved commit".into(),
            ));
        }
        Ok(PullRequest {
            external_id: value["node_id"].as_str().unwrap_or_default().into(),
            url: value["html_url"].as_str().unwrap_or_default().into(),
            number: value["number"].as_u64().unwrap_or_default(),
            draft: value["draft"].as_bool().unwrap_or(true),
            head_oid,
        })
    }

    async fn write_back(&self, request: WriteBack) -> Result<(), ConnectorError> {
        if request.expected_issue_version.is_empty() {
            return Err(ConnectorError::Configuration(
                "expected issue version is required".into(),
            ));
        }
        let body = format!(
            "{}\n\nEvidence:\n{}\n\n<!-- opencoding:{} -->",
            request.message,
            request
                .evidence_urls
                .iter()
                .map(|url| format!("- {url}"))
                .collect::<Vec<_>>()
                .join("\n"),
            request.context.idempotency_key
        );
        let response = self
            .request(
                Method::POST,
                &format!(
                    "/repos/{}/issues/{}/comments",
                    self.repository, request.issue_number
                ),
                &request.context,
                Some(json!({"body": body})),
            )
            .await?;
        if response.status() != StatusCode::CREATED && !response.status().is_success() {
            return Err(ConnectorError::Request(response.status().to_string()));
        }
        Ok(())
    }
}

pub fn verify_github_webhook(
    secret: &[u8],
    signature: &str,
    body: &[u8],
) -> Result<(), ConnectorError> {
    let supplied = signature
        .strip_prefix("sha256=")
        .ok_or(ConnectorError::InvalidWebhookSignature)?;
    let bytes = decode_hex(supplied).ok_or(ConnectorError::InvalidWebhookSignature)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| ConnectorError::InvalidWebhookSignature)?;
    mac.update(body);
    mac.verify_slice(&bytes)
        .map_err(|_| ConnectorError::InvalidWebhookSignature)
}

fn pull_request_body(request: &CreateDraftPullRequest) -> String {
    format!(
        "## Summary\n{}\n\n## Risk\n{}\n\n## Verification\n{}\n\n## Suggested reviewers\n{}\n\n## Agent evidence\n- Team: `{}`\n- Task: `{}`\n- Session: `{}`\n- Model: `{}`\n- Approved diff: `{}`\n- Approved commit: `{}`\n- Audit events: {}\n",
        request.evidence.summary,
        request.evidence.risk,
        request
            .evidence
            .tests
            .iter()
            .map(|test| format!("- [x] {test}"))
            .collect::<Vec<_>>()
            .join("\n"),
        if request.evidence.reviewer_suggestions.is_empty() {
            "- Team review queue".into()
        } else {
            request
                .evidence
                .reviewer_suggestions
                .iter()
                .map(|reviewer| format!("- {reviewer}"))
                .collect::<Vec<_>>()
                .join("\n")
        },
        request.context.scope.team_id.0,
        request.context.task_id.0,
        request.context.session_id.0,
        request.evidence.model,
        request.evidence.approved_diff_hash,
        request.expected_head_oid,
        request
            .evidence
            .audit_event_ids
            .iter()
            .map(|id| format!("`{}`", id.0))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn validate_action_context(context: &ActionContext) -> Result<(), ConnectorError> {
    if context.idempotency_key.len() < 8
        || context.actor_reason.trim().is_empty()
        || context.task_id.0.is_empty()
        || context.session_id.0.is_empty()
    {
        return Err(ConnectorError::Configuration(
            "write/read context requires task, session, reason, and idempotency key".into(),
        ));
    }
    Ok(())
}

fn bounded_identifier(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:/@*?-".contains(character))
}

fn validate_repository_slug(repository: &str) -> Result<(), ConnectorError> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || !bounded_identifier(owner, 128)
        || !bounded_identifier(name, 128)
        || owner.contains("..")
        || name.contains("..")
    {
        return Err(ConnectorError::Configuration(
            "repository must be a bounded owner/name slug".into(),
        ));
    }
    Ok(())
}

fn validate_notification(request: &TeamNotification) -> Result<(), ConnectorError> {
    validate_action_context(&request.context)?;
    if !bounded_identifier(&request.channel, 256) {
        return Err(ConnectorError::Configuration(
            "notification requires bounded channel/message and credential-free HTTPS evidence URLs"
                .into(),
        ));
    }
    validate_connector_text(
        &request.message,
        &request.evidence_urls,
        "notification",
        4_000,
    )
}

fn validate_connector_text(
    message: &str,
    evidence_urls: &[String],
    label: &str,
    max_message_bytes: usize,
) -> Result<(), ConnectorError> {
    if message.trim().is_empty()
        || message.len() > max_message_bytes
        || message.contains('\0')
        || evidence_urls.len() > 32
        || evidence_urls.iter().any(|value| {
            url::Url::parse(value).ok().is_none_or(|url| {
                url.scheme() != "https"
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || value.len() > 2048
            })
        })
    {
        return Err(ConnectorError::Configuration(format!(
            "{label} requires bounded text and credential-free HTTPS evidence URLs"
        )));
    }
    Ok(())
}

fn validate_servicenow_sys_id(value: &str) -> Result<(), ConnectorError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConnectorError::Configuration(
            "ServiceNow sys_id must be exactly 32 hexadecimal characters".into(),
        ));
    }
    Ok(())
}

fn notification_text(request: &TeamNotification) -> String {
    let evidence = request
        .evidence_urls
        .iter()
        .map(|value| format!("- {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    if evidence.is_empty() {
        request.message.clone()
    } else {
        format!("{}\n\nEvidence:\n{evidence}", request.message)
    }
}

fn validated_webhook_url(value: &str) -> Result<String, ConnectorError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| ConnectorError::Credential("invalid Teams webhook URL".into()))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if (parsed.scheme() != "https" && !loopback)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || value.len() > 4096
    {
        return Err(ConnectorError::Credential(
            "Teams webhook must use HTTPS and contain no embedded credentials".into(),
        ));
    }
    Ok(value.into())
}

async fn response_json(response: reqwest::Response) -> Result<Value, ConnectorError> {
    let status = response.status();
    if !status.is_success() {
        return Err(ConnectorError::Request(status.to_string()));
    }
    let body = bounded_response_body(response, 1024 * 1024, "connector").await?;
    serde_json::from_slice(&body).map_err(|error| ConnectorError::Response(error.to_string()))
}

fn connector_http_client() -> Result<reqwest::Client, ConnectorError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| ConnectorError::Configuration(error.to_string()))
}

async fn bounded_response_body(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, ConnectorError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(ConnectorError::Response(format!(
            "{label} response exceeds {limit} bytes"
        )));
    }
    let mut body =
        Vec::with_capacity(response.content_length().unwrap_or(0).min(limit as u64) as usize);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ConnectorError::Response(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ConnectorError::Response(format!(
                "{label} response exceeds {limit} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn ensure_success(response: reqwest::Response) -> Result<(), ConnectorError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    Err(ConnectorError::Request(status.to_string()))
}

fn validated_api_base(api_base: String) -> Result<String, ConnectorError> {
    let api_base = api_base.trim_end_matches('/').to_owned();
    let parsed = url::Url::parse(&api_base)
        .map_err(|error| ConnectorError::Configuration(error.to_string()))?;
    let loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ConnectorError::Configuration(
            "connector API base must not contain credentials, query, or fragment".into(),
        ));
    }
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(ConnectorError::Configuration(
            "connector API must use HTTPS unless it is loopback".into(),
        ));
    }
    Ok(api_base)
}

fn validate_branch(value: &str) -> Result<(), ConnectorError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains(char::is_whitespace)
        || value.contains("..")
    {
        return Err(ConnectorError::Configuration("invalid branch".into()));
    }
    Ok(())
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use opencoding_git_automation::{CommitRequest, GitService};
    use opencoding_identity::{ConnectorApprovalClaims, TeamGrantSigner};
    use std::{
        process::Command,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    type CapturedRequest = Arc<Mutex<Option<(HeaderMap, Value)>>>;

    #[derive(Clone)]
    struct GitE2eState {
        captured: CapturedRequest,
        head_oid: String,
    }

    struct TestBroker;

    struct FixedBroker(String);

    #[async_trait]
    impl CredentialBroker for TestBroker {
        async fn issue(&self, _: &str, _: &ActionContext) -> Result<String, ConnectorError> {
            Ok("short-lived-token".into())
        }
    }

    #[async_trait]
    impl CredentialBroker for FixedBroker {
        async fn issue(&self, _: &str, _: &ActionContext) -> Result<String, ConnectorError> {
            Ok(self.0.clone())
        }
    }

    fn action_context() -> ActionContext {
        ActionContext {
            scope: Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("user".into()),
                goal_id: None,
                task_id: Some(Id("task".into())),
            },
            session_id: Id("session".into()),
            task_id: Id("task".into()),
            actor_reason: "deliver task".into(),
            idempotency_key: "idem-123456".into(),
        }
    }

    #[tokio::test]
    async fn slack_teams_github_actions_and_splunk_contracts_use_scoped_brokered_http() {
        type Captures = Arc<Mutex<Vec<(String, HeaderMap, Value)>>>;
        let captures: Captures = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/api/chat.postMessage",
                axum::routing::post(
                    |State(captures): State<Captures>, headers: HeaderMap, Json(body): Json<Value>| async move {
                        captures.lock().unwrap().push(("slack".into(), headers, body));
                        Json(json!({"ok":true,"ts":"1"}))
                    },
                ),
            )
            .route(
                "/teams-webhook",
                axum::routing::post(
                    |State(captures): State<Captures>, headers: HeaderMap, Json(body): Json<Value>| async move {
                        captures.lock().unwrap().push(("teams".into(), headers, body));
                        StatusCode::ACCEPTED
                    },
                ),
            )
            .route(
                "/repos/acme/repo/commits/abcdef123456/check-runs",
                get(
                    |State(captures): State<Captures>, headers: HeaderMap| async move {
                        captures.lock().unwrap().push(("ci".into(), headers, json!({})));
                        Json(json!({"check_runs":[{"name":"build","status":"completed","conclusion":"success","details_url":"https://ci.example/build/1"}]}))
                    },
                ),
            )
            .route(
                "/services/collector/event",
                axum::routing::post(
                    |State(captures): State<Captures>, headers: HeaderMap, Json(body): Json<Value>| async move {
                        captures.lock().unwrap().push(("siem".into(), headers, body));
                        Json(json!({"text":"Success","code":0}))
                    },
                ),
            )
            .with_state(captures.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{address}");
        let notification = TeamNotification {
            context: action_context(),
            channel: "C-TEAM".into(),
            message: "Verified Draft PR is ready".into(),
            evidence_urls: vec!["https://scm.example/pull/7".into()],
        };
        SlackConnector::new(&base, "SLACK_TOKEN", Arc::new(TestBroker))
            .unwrap()
            .send_notification(notification.clone())
            .await
            .unwrap();
        TeamsConnector::new(
            "TEAMS_WEBHOOK",
            Arc::new(FixedBroker(format!("{base}/teams-webhook"))),
        )
        .unwrap()
        .send_notification(notification)
        .await
        .unwrap();
        let checks =
            GitHubActionsConnector::new(&base, "acme/repo", "GITHUB_TOKEN", Arc::new(TestBroker))
                .unwrap()
                .read_commit_checks(CommitChecksRequest {
                    context: action_context(),
                    commit_oid: "abcdef123456".into(),
                })
                .await
                .unwrap();
        assert_eq!(checks[0].conclusion.as_deref(), Some("success"));
        SplunkHecConnector::new(&base, "SPLUNK_TOKEN", Arc::new(TestBroker))
            .unwrap()
            .export_event(SecurityEvent {
                context: action_context(),
                event_type: "policy.denied".into(),
                severity: "high".into(),
                resource_type: "tool".into(),
                resource_id: Id("tool-call-7".into()),
                evidence_ids: vec![Id("audit-7".into())],
                occurred_at: "2026-07-21T00:00:00Z".into(),
            })
            .await
            .unwrap();
        let captured = captures.lock().unwrap();
        assert_eq!(captured.len(), 4);
        for (_, headers, _) in captured.iter() {
            assert_eq!(headers["x-opencoding-team"], "team");
        }
        assert_eq!(captured[0].1["authorization"], "Bearer short-lived-token");
        assert_eq!(captured[3].1["authorization"], "Splunk short-lived-token");
        assert_eq!(captured[3].2["event"]["code_content_present"], false);
        assert!(!captured[3].2.to_string().contains("Verified Draft PR"));
        server.abort();
    }

    #[test]
    fn connector_api_bases_reject_embedded_credentials_and_ambiguous_urls() {
        for accepted in [
            "https://api.example/v1",
            "http://127.0.0.1:9000/v1",
            "http://localhost:9000/v1",
        ] {
            assert!(
                validated_api_base(accepted.into()).is_ok(),
                "rejected {accepted}"
            );
        }
        for rejected in [
            "http://api.example/v1",
            "https://user:password@api.example/v1",
            "https://api.example/v1?access_token=secret",
            "https://api.example/v1#fragment",
        ] {
            assert!(
                validated_api_base(rejected.into()).is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn webhook_signature_is_constant_time_verified() {
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(b"payload");
        let signature = format!(
            "sha256={}",
            mac.finalize()
                .into_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        verify_github_webhook(b"secret", &signature, b"payload").unwrap();
        assert!(verify_github_webhook(b"secret", &signature, b"changed").is_err());
    }

    #[test]
    fn pull_request_body_carries_team_and_verification_evidence() {
        let request = CreateDraftPullRequest {
            context: action_context(),
            title: "change".into(),
            base: "main".into(),
            head: "opencoding/task".into(),
            expected_head_oid: "0123456789abcdef".into(),
            evidence: PullRequestEvidence {
                summary: "summary".into(),
                risk: "low".into(),
                tests: vec!["cargo test".into()],
                model: "model".into(),
                approved_diff_hash: "a".repeat(64),
                audit_event_ids: vec![Id("evt".into())],
                reviewer_suggestions: vec!["@platform/team".into()],
            },
        };
        let body = pull_request_body(&request);
        assert!(body.contains("Team: `team`"));
        assert!(body.contains("- [x] cargo test"));
        assert!(body.contains("Approved commit: `0123456789abcdef`"));
        assert!(body.contains("@platform/team"));
    }

    #[tokio::test]
    async fn github_contract_creates_draft_with_short_lived_token_and_idempotency() {
        let captured = Arc::new(Mutex::new(None));
        let capture = captured.clone();
        let app = Router::new()
            .route(
                "/repos/acme/repo/pulls",
                get(|| async { Json(json!([])) }).post(
                    |State(capture): State<CapturedRequest>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *capture.lock().unwrap() = Some((headers, body));
                        (
                            StatusCode::CREATED,
                            Json(json!({
                                "node_id":"PR_1",
                                "html_url":"https://github.example/acme/repo/pull/1",
                                "number":1,
                                "draft":true,
                                "head":{"sha":"0123456789abcdef"}
                            })),
                        )
                    },
                ),
            )
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let connector = GitHubEnterpriseConnector::new(
            format!("http://{address}"),
            "acme/repo",
            "github-installation",
            Arc::new(TestBroker),
        )
        .unwrap();
        let pull_request = connector
            .create_draft_pr(CreateDraftPullRequest {
                context: action_context(),
                title: "Verified change".into(),
                base: "main".into(),
                head: "opencoding/task".into(),
                expected_head_oid: "0123456789abcdef".into(),
                evidence: PullRequestEvidence {
                    summary: "summary".into(),
                    risk: "low".into(),
                    tests: vec!["cargo test".into()],
                    model: "model".into(),
                    approved_diff_hash: "a".repeat(64),
                    audit_event_ids: vec![Id("evt".into())],
                    reviewer_suggestions: vec!["@platform/team".into()],
                },
            })
            .await
            .unwrap();
        assert!(pull_request.draft);
        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["authorization"], "Bearer short-lived-token");
        assert_eq!(headers["x-opencoding-team"], "team");
        assert_eq!(headers["x-opencoding-idempotency-key"], "idem-123456");
        assert_eq!(body["draft"], true);
        assert!(body["body"].as_str().unwrap().contains("cargo test"));
    }

    #[tokio::test]
    async fn jira_contract_reads_version_and_writes_evidence_with_concurrency_guard() {
        let captured: CapturedRequest = Arc::new(Mutex::new(None));
        let capture = captured.clone();
        let app = Router::new()
            .route(
                "/rest/api/3/issue/OPS-42",
                get(|| async {
                    (
                        [("etag", "issue-v7")],
                        Json(json!({
                            "id":"10042",
                            "fields":{
                                "summary":"Repair deployment",
                                "description":"Acceptance criteria",
                                "status":{"name":"In Progress"}
                            }
                        })),
                    )
                }),
            )
            .route(
                "/rest/api/3/issue/OPS-42/comment",
                axum::routing::post(
                    |State(capture): State<CapturedRequest>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *capture.lock().unwrap() = Some((headers, body));
                        (StatusCode::CREATED, Json(json!({"id":"comment-1"})))
                    },
                ),
            )
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let connector = JiraWorkManagementConnector::new(
            format!("http://{address}"),
            "OPS",
            "jira-oauth",
            Arc::new(TestBroker),
        )
        .unwrap();
        let context = action_context();
        let issue = connector.read_task(42, &context).await.unwrap();
        assert_eq!(issue.external_id, "10042");
        assert_eq!(issue.version, "issue-v7");
        connector
            .write_back(WriteBack {
                context,
                issue_number: 42,
                expected_issue_version: issue.version,
                message: "Draft PR is ready".into(),
                evidence_urls: vec!["https://github.example/acme/repo/pull/7".into()],
            })
            .await
            .unwrap();
        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["authorization"], "Bearer short-lived-token");
        assert_eq!(headers["if-match"], "issue-v7");
        assert_eq!(headers["x-opencoding-team"], "team");
        assert_eq!(headers["x-opencoding-idempotency-key"], "idem-123456");
        let serialized = body.to_string();
        assert!(serialized.contains("Draft PR is ready"));
        assert!(serialized.contains("pull/7"));
        assert!(serialized.contains("opencoding:idem-123456"));
        server.abort();
    }

    #[tokio::test]
    async fn servicenow_contract_requires_signed_four_eyes_and_replays_remote_marker() {
        #[derive(Clone)]
        struct SnowState {
            captured: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
            written: Arc<AtomicBool>,
        }
        let state = SnowState {
            captured: Arc::new(Mutex::new(Vec::new())),
            written: Arc::new(AtomicBool::new(false)),
        };
        let app = Router::new()
            .route(
                "/api/now/v1/table/sys_journal_field",
                get(
                    |State(state): State<SnowState>, headers: HeaderMap| async move {
                        state
                            .captured
                            .lock()
                            .unwrap()
                            .push((headers, json!({"operation":"journal_lookup"})));
                        Json(if state.written.load(Ordering::SeqCst) {
                            json!({"result":[{"sys_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]})
                        } else {
                            json!({"result":[]})
                        })
                    },
                ),
            )
            .route(
                "/api/now/v1/table/incident/0123456789abcdef0123456789abcdef",
                get(
                    |State(state): State<SnowState>, headers: HeaderMap| async move {
                        state
                            .captured
                            .lock()
                            .unwrap()
                            .push((headers, json!({"operation":"record_read"})));
                        Json(json!({"result":{
                            "sys_id":"0123456789abcdef0123456789abcdef",
                            "number":"INC0000042","short_description":"Deployment failed",
                            "description":"Investigate rollout","state":"2",
                            "sys_updated_on":"2026-07-22 10:00:00","sys_mod_count":"7"
                        }}))
                    },
                )
                .patch(
                    |State(state): State<SnowState>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        state.written.store(true, Ordering::SeqCst);
                        state.captured.lock().unwrap().push((headers, body));
                        Json(json!({"result":{
                            "sys_id":"0123456789abcdef0123456789abcdef",
                            "number":"INC0000042","short_description":"Deployment failed",
                            "description":"Investigate rollout","state":"2",
                            "sys_updated_on":"2026-07-22 10:01:00","sys_mod_count":"8"
                        }}))
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let signer =
            TeamGrantSigner::from_base64("grant-key", &URL_SAFE_NO_PAD.encode([31_u8; 32]))
                .unwrap();
        let verifier = TeamGrantVerifier::from_base64(
            "grant-key",
            &signer.public_key_base64(),
            "control-plane",
            "daemon",
        )
        .unwrap();
        let context = action_context();
        let now = chrono::Utc::now();
        let grant = signer
            .sign_connector_approval(&ConnectorApprovalClaims {
                approval_id: Id("central-approval-42".into()),
                issuer: "control-plane".into(),
                audience: "daemon".into(),
                organization_id: context.scope.organization_id.clone(),
                team_id: context.scope.team_id.clone(),
                requested_by: context.scope.actor_id.clone(),
                approved_by: Id("approver".into()),
                resource_type: "connector_write".into(),
                resource_id: Id("0123456789abcdef0123456789abcdef".into()),
                action: "servicenow.append_work_note".into(),
                idempotency_key: context.idempotency_key.clone(),
                issued_at: now,
                not_before: now - chrono::Duration::seconds(1),
                expires_at: now + chrono::Duration::minutes(5),
            })
            .unwrap();
        let connector = ServiceNowConnector::new(
            format!("http://{address}"),
            "incident",
            "SERVICENOW_TOKEN",
            Arc::new(TestBroker),
            verifier,
        )
        .unwrap();
        let request = ServiceNowWorkNote {
            context: context.clone(),
            record_sys_id: "0123456789abcdef0123456789abcdef".into(),
            expected_mod_count: 7,
            work_note: "Verified rollback completed".into(),
            evidence_urls: vec!["https://evidence.example/rollback/42".into()],
            approval_grant: grant.clone(),
        };
        let result = connector.append_work_note(request.clone()).await.unwrap();
        assert_eq!(result.sys_mod_count, 8);
        assert!(!result.replayed);
        let replay = connector.append_work_note(request).await.unwrap();
        assert!(replay.replayed);
        {
            let captured = state.captured.lock().unwrap();
            assert_eq!(captured.len(), 4);
            let (headers, body) = &captured[2];
            assert_eq!(headers["authorization"], "Bearer short-lived-token");
            assert_eq!(headers["x-opencoding-team"], "team");
            assert_eq!(headers["x-opencoding-approval-id"], "central-approval-42");
            let note = body["work_notes"].as_str().unwrap();
            assert!(note.contains("Verified rollback completed"));
            assert!(note.contains("opencoding:idem-123456"));
            assert!(note.contains("approval:central-approval-42"));
        }

        let mut tampered = grant.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        let before = state.captured.lock().unwrap().len();
        let rejected = connector
            .append_work_note(ServiceNowWorkNote {
                context,
                record_sys_id: "0123456789abcdef0123456789abcdef".into(),
                expected_mod_count: 8,
                work_note: "must not execute".into(),
                evidence_urls: vec![],
                approval_grant: String::from_utf8(tampered).unwrap(),
            })
            .await;
        assert!(rejected.is_err());
        assert_eq!(state.captured.lock().unwrap().len(), before);
        server.abort();
    }

    #[tokio::test]
    async fn git_branch_commit_push_and_draft_pr_are_one_verified_chain() {
        let repository = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let run_git = |directory: &std::path::Path, args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(directory)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        run_git(repository.path(), &["init", "-b", "main"]);
        std::fs::write(
            repository.path().join("service.rs"),
            "fn version() -> u8 { 1 }\n",
        )
        .unwrap();
        run_git(repository.path(), &["add", "service.rs"]);
        run_git(
            repository.path(),
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );
        run_git(remote.path(), &["init", "--bare"]);
        run_git(
            repository.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        let repository_uri = url::Url::from_directory_path(repository.path()).unwrap();
        let root = GitService::open(repository_uri.as_str()).unwrap();
        let worktree = root
            .create_worktree("main", "opencoding/team-task")
            .unwrap();
        let worktree_path = url::Url::parse(&worktree.workspace_uri)
            .unwrap()
            .to_file_path()
            .unwrap();
        std::fs::write(
            worktree_path.join("service.rs"),
            "fn version() -> u8 { 2 }\n",
        )
        .unwrap();
        let git = GitService::open(&worktree.workspace_uri).unwrap();
        let diff = git.diff(&["service.rs".into()], 64 * 1024).unwrap();
        assert!(!diff.truncated);
        let context = action_context();
        let commit = git
            .commit(CommitRequest {
                message: "Upgrade service version".into(),
                paths: vec!["service.rs".into()],
                expected_diff_hash: diff.sha256.clone(),
                scope: context.scope.clone(),
                session_id: context.session_id.clone(),
            })
            .unwrap();
        run_git(
            &worktree_path,
            &["push", "origin", "HEAD:refs/heads/opencoding/team-task"],
        );
        let remote_head = run_git(
            remote.path(),
            &["rev-parse", "refs/heads/opencoding/team-task"],
        );
        assert_eq!(
            String::from_utf8_lossy(&remote_head.stdout).trim(),
            commit.oid
        );

        let captured: CapturedRequest = Arc::new(Mutex::new(None));
        let mock_state = GitE2eState {
            captured: captured.clone(),
            head_oid: commit.oid.clone(),
        };
        let mock = Router::new()
            .route(
                "/repos/acme/repo/pulls",
                get(|| async { Json(json!([])) }).post(
                    |State(state): State<GitE2eState>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *state.captured.lock().unwrap() = Some((headers, body));
                        (
                            StatusCode::CREATED,
                            Json(json!({
                                "node_id":"PR_CHAIN",
                                "html_url":"https://github.example/acme/repo/pull/7",
                                "number":7,
                                "draft":true,
                                "head":{"sha":state.head_oid}
                            })),
                        )
                    },
                ),
            )
            .with_state(mock_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, mock).await.unwrap() });
        let connector = GitHubEnterpriseConnector::new(
            format!("http://{address}"),
            "acme/repo",
            "github-installation",
            Arc::new(TestBroker),
        )
        .unwrap();
        let pull_request = connector
            .create_draft_pr(CreateDraftPullRequest {
                context,
                title: "Upgrade service version".into(),
                base: "main".into(),
                head: commit.branch.clone(),
                expected_head_oid: commit.oid.clone(),
                evidence: PullRequestEvidence {
                    summary: "One-file approved change".into(),
                    risk: "low".into(),
                    tests: vec!["cargo test".into()],
                    model: "fixture-agent-v1".into(),
                    approved_diff_hash: diff.sha256.clone(),
                    audit_event_ids: vec![Id("evt_verified".into())],
                    reviewer_suggestions: vec!["@service/owners".into()],
                },
            })
            .await
            .unwrap();
        assert!(pull_request.draft);
        assert_eq!(pull_request.head_oid, commit.oid);
        let (headers, body) = captured.lock().unwrap().take().unwrap();
        assert_eq!(headers["authorization"], "Bearer short-lived-token");
        assert_eq!(body["head"], "opencoding/team-task");
        assert_eq!(body["draft"], true);
        assert!(body["body"].as_str().unwrap().contains(&diff.sha256));
        assert!(body["body"].as_str().unwrap().contains("cargo test"));
        assert!(body["body"].as_str().unwrap().contains("@service/owners"));

        let ticket_capture: CapturedRequest = Arc::new(Mutex::new(None));
        let ticket_state = ticket_capture.clone();
        let ticket_mock = Router::new()
            .route(
                "/rest/api/3/issue/OPS-7/comment",
                axum::routing::post(
                    |State(capture): State<CapturedRequest>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        *capture.lock().unwrap() = Some((headers, body));
                        (StatusCode::CREATED, Json(json!({"id":"comment-chain"})))
                    },
                ),
            )
            .with_state(ticket_state);
        let ticket_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ticket_address = ticket_listener.local_addr().unwrap();
        let ticket_server =
            tokio::spawn(async move { axum::serve(ticket_listener, ticket_mock).await.unwrap() });
        let ticket_connector = JiraWorkManagementConnector::new(
            format!("http://{ticket_address}"),
            "OPS",
            "jira-oauth",
            Arc::new(TestBroker),
        )
        .unwrap();
        let mut ticket_context = action_context();
        ticket_context.idempotency_key = "task-7-pr-7-writeback".into();
        ticket_connector
            .write_back(WriteBack {
                context: ticket_context,
                issue_number: 7,
                expected_issue_version: "issue-v3".into(),
                message: "Verified Draft PR is ready for Team review".into(),
                evidence_urls: vec![pull_request.url.clone()],
            })
            .await
            .unwrap();
        let (ticket_headers, ticket_body) = ticket_capture.lock().unwrap().take().unwrap();
        assert_eq!(ticket_headers["if-match"], "issue-v3");
        assert_eq!(ticket_headers["x-opencoding-team"], "team");
        assert!(ticket_body.to_string().contains(&pull_request.url));
        assert!(ticket_body.to_string().contains("task-7-pr-7-writeback"));
        ticket_server.abort();
        server.abort();
    }
}
