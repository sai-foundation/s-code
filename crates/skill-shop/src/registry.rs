//! The registry boundary: one small trait every skill backend implements,
//! and the HTTP client for the remote registry service. The local SQLite
//! backend lives in the daemon; the remote service lives in
//! `s-code-skill-registry`. Both apply the same domain rules.
use crate::domain::{
    ChallengeItem, ChallengeSubmission, ComparisonAccepted, ComparisonItem, ComparisonSubmission,
    ForkSubmission, Lineage, SkillArtifact, SkillPublication, SkillReceiptItem,
    SkillReceiptSubmission, SkillStatus,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

pub const MAX_REGISTRY_RESPONSE_BYTES: usize = 1024 * 1024;
pub const REGISTRY_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry authentication failed")]
    Unauthorized,
    #[error("registry refused the request")]
    Forbidden,
    #[error("skill not found")]
    NotFound,
    #[error("registry conflict: {0}")]
    Conflict(String),
    #[error("registry rejected the request: {0}")]
    Invalid(String),
    #[error("registry unavailable: {0}")]
    Unavailable(String),
    #[error("registry response is not usable: {0}")]
    Malformed(String),
}

/// Listing filters; every backend applies its own visibility rules first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillQuery {
    pub status: Option<SkillStatus>,
    pub q: Option<String>,
    pub task_family: Option<String>,
    pub model: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Published {
    pub skill: SkillArtifact,
    pub created: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReceiptAccepted {
    pub receipt: SkillReceiptItem,
    pub skill: SkillArtifact,
    pub transition: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeprecateRequest {
    pub reason: String,
}

/// The skill lifecycle as every backend exposes it. Identity comes from the
/// backend's authenticated caller, never from these arguments.
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    async fn publish(&self, publication: SkillPublication) -> Result<Published, RegistryError>;
    async fn get(&self, id: &str) -> Result<SkillArtifact, RegistryError>;
    async fn list(&self, query: &SkillQuery) -> Result<Vec<SkillArtifact>, RegistryError>;
    async fn submit_receipt(
        &self,
        id: &str,
        receipt: SkillReceiptSubmission,
    ) -> Result<ReceiptAccepted, RegistryError>;
    async fn list_receipts(&self, id: &str) -> Result<Vec<SkillReceiptItem>, RegistryError>;
    async fn deprecate(&self, id: &str, reason: &str) -> Result<SkillArtifact, RegistryError>;
    // -- forum --
    async fn submit_challenge(
        &self,
        id: &str,
        challenge: ChallengeSubmission,
    ) -> Result<(ChallengeItem, bool), RegistryError>;
    async fn list_challenges(&self, id: &str) -> Result<Vec<ChallengeItem>, RegistryError>;
    async fn fork(&self, id: &str, fork: ForkSubmission) -> Result<Published, RegistryError>;
    async fn list_forks(&self, id: &str) -> Result<Vec<SkillArtifact>, RegistryError>;
    async fn lineage(&self, id: &str) -> Result<Lineage, RegistryError>;
    async fn submit_comparison(
        &self,
        fork_id: &str,
        comparison: ComparisonSubmission,
    ) -> Result<ComparisonAccepted, RegistryError>;
    async fn list_comparisons(&self, fork_id: &str) -> Result<Vec<ComparisonItem>, RegistryError>;
}

/// Where the client obtains its bearer token at request time. The token is
/// never stored in configuration, logs or events.
#[async_trait]
pub trait RegistryTokenSource: Send + Sync {
    async fn token(&self) -> Result<String, RegistryError>;
}

/// The repository's credential-handle convention: a handle names an
/// environment variable holding the secret.
pub struct EnvironmentTokenSource {
    pub handle: String,
}

#[async_trait]
impl RegistryTokenSource for EnvironmentTokenSource {
    async fn token(&self) -> Result<String, RegistryError> {
        match std::env::var(&self.handle) {
            Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
            _ => Err(RegistryError::Unauthorized),
        }
    }
}

/// A fixed token, for tests and one-process population experiments.
pub struct StaticTokenSource(pub String);

#[async_trait]
impl RegistryTokenSource for StaticTokenSource {
    async fn token(&self) -> Result<String, RegistryError> {
        Ok(self.0.clone())
    }
}

/// A registry URL must be HTTPS, or plain HTTP on loopback only, without
/// credentials, query or fragment.
pub fn validate_registry_url(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|error| error.to_string())?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "the skill registry URL must be HTTPS, or loopback HTTP, without credentials, query or fragment".into(),
        );
    }
    Ok(value.trim_end_matches('/').to_owned())
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
}

/// HTTP client for the remote registry. Bounded responses, short timeouts,
/// no redirects, bearer authentication on every request, and fail-closed
/// decoding: anything the registry returns that does not parse as the
/// expected contract is an error, never partial data.
pub struct RemoteSkillRegistryClient {
    client: reqwest::Client,
    base_url: String,
    tokens: Arc<dyn RegistryTokenSource>,
}

impl RemoteSkillRegistryClient {
    pub fn new(base_url: &str, tokens: Arc<dyn RegistryTokenSource>) -> Result<Self, String> {
        let base_url = validate_registry_url(base_url)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // Environment proxies never see the bearer: the registry is
            // reached directly at its validated origin.
            .no_proxy()
            .timeout(REGISTRY_REQUEST_TIMEOUT)
            .user_agent(concat!("s-code-skill-shop/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            base_url,
            tokens,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<(reqwest::StatusCode, T), RegistryError> {
        let token = self.tokens.token().await?;
        let mut request = self
            .client
            .request(method, format!("{}{}", self.base_url, path))
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| RegistryError::Unavailable(error.to_string()))?;
        let status = response.status();
        let bytes = bounded_body(response).await?;
        if !status.is_success() {
            let detail = serde_json::from_slice::<ErrorBody>(&bytes)
                .ok()
                .and_then(|body| body.error)
                .unwrap_or_else(|| status.to_string());
            return Err(match status.as_u16() {
                401 => RegistryError::Unauthorized,
                403 => RegistryError::Forbidden,
                404 => RegistryError::NotFound,
                409 => RegistryError::Conflict(detail),
                400 | 422 => RegistryError::Invalid(detail),
                _ => RegistryError::Unavailable(detail),
            });
        }
        let value = serde_json::from_slice::<T>(&bytes)
            .map_err(|error| RegistryError::Malformed(error.to_string()))?;
        Ok((status, value))
    }
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, RegistryError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REGISTRY_RESPONSE_BYTES as u64)
    {
        return Err(RegistryError::Malformed("response too large".into()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| RegistryError::Unavailable(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_REGISTRY_RESPONSE_BYTES {
            return Err(RegistryError::Malformed("response too large".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[async_trait]
impl SkillRegistry for RemoteSkillRegistryClient {
    async fn publish(&self, publication: SkillPublication) -> Result<Published, RegistryError> {
        let body = serde_json::to_value(&publication)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (status, skill) = self
            .send::<SkillArtifact>(reqwest::Method::POST, "/v1/skills", Some(body))
            .await?;
        Ok(Published {
            skill,
            created: status == reqwest::StatusCode::CREATED,
        })
    }

    async fn get(&self, id: &str) -> Result<SkillArtifact, RegistryError> {
        let (_, skill) = self
            .send::<SkillArtifact>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}", encode(id)),
                None,
            )
            .await?;
        Ok(skill)
    }

    async fn list(&self, query: &SkillQuery) -> Result<Vec<SkillArtifact>, RegistryError> {
        let mut pairs = Vec::new();
        if let Some(status) = query.status {
            pairs.push(("status", status.as_str().to_owned()));
        }
        for (key, value) in [
            ("q", &query.q),
            ("task_family", &query.task_family),
            ("model", &query.model),
        ] {
            if let Some(value) = value {
                pairs.push((key, value.clone()));
            }
        }
        if let Some(limit) = query.limit {
            pairs.push(("limit", limit.to_string()));
        }
        if let Some(offset) = query.offset {
            pairs.push(("offset", offset.to_string()));
        }
        let query_string = pairs
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let path = if query_string.is_empty() {
            "/v1/skills".to_owned()
        } else {
            format!("/v1/skills?{query_string}")
        };
        let (_, skills) = self
            .send::<Vec<SkillArtifact>>(reqwest::Method::GET, &path, None)
            .await?;
        Ok(skills)
    }

    async fn submit_receipt(
        &self,
        id: &str,
        receipt: SkillReceiptSubmission,
    ) -> Result<ReceiptAccepted, RegistryError> {
        let body = serde_json::to_value(&receipt)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (_, accepted) = self
            .send::<ReceiptAccepted>(
                reqwest::Method::POST,
                &format!("/v1/skills/{}/evaluations", encode(id)),
                Some(body),
            )
            .await?;
        Ok(accepted)
    }

    async fn list_receipts(&self, id: &str) -> Result<Vec<SkillReceiptItem>, RegistryError> {
        let (_, receipts) = self
            .send::<Vec<SkillReceiptItem>>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}/evaluations", encode(id)),
                None,
            )
            .await?;
        Ok(receipts)
    }

    async fn submit_challenge(
        &self,
        id: &str,
        challenge: ChallengeSubmission,
    ) -> Result<(ChallengeItem, bool), RegistryError> {
        let body = serde_json::to_value(&challenge)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (status, item) = self
            .send::<ChallengeItem>(
                reqwest::Method::POST,
                &format!("/v1/skills/{}/challenges", encode(id)),
                Some(body),
            )
            .await?;
        Ok((item, status == reqwest::StatusCode::CREATED))
    }

    async fn list_challenges(&self, id: &str) -> Result<Vec<ChallengeItem>, RegistryError> {
        let (_, items) = self
            .send::<Vec<ChallengeItem>>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}/challenges", encode(id)),
                None,
            )
            .await?;
        Ok(items)
    }

    async fn fork(&self, id: &str, fork: ForkSubmission) -> Result<Published, RegistryError> {
        let body = serde_json::to_value(&fork)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (status, skill) = self
            .send::<SkillArtifact>(
                reqwest::Method::POST,
                &format!("/v1/skills/{}/forks", encode(id)),
                Some(body),
            )
            .await?;
        Ok(Published {
            skill,
            created: status == reqwest::StatusCode::CREATED,
        })
    }

    async fn list_forks(&self, id: &str) -> Result<Vec<SkillArtifact>, RegistryError> {
        let (_, items) = self
            .send::<Vec<SkillArtifact>>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}/forks", encode(id)),
                None,
            )
            .await?;
        Ok(items)
    }

    async fn lineage(&self, id: &str) -> Result<Lineage, RegistryError> {
        let (_, lineage) = self
            .send::<Lineage>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}/lineage", encode(id)),
                None,
            )
            .await?;
        Ok(lineage)
    }

    async fn submit_comparison(
        &self,
        fork_id: &str,
        comparison: ComparisonSubmission,
    ) -> Result<ComparisonAccepted, RegistryError> {
        let body = serde_json::to_value(&comparison)
            .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (_, accepted) = self
            .send::<ComparisonAccepted>(
                reqwest::Method::POST,
                &format!("/v1/skills/{}/comparisons", encode(fork_id)),
                Some(body),
            )
            .await?;
        Ok(accepted)
    }

    async fn list_comparisons(&self, fork_id: &str) -> Result<Vec<ComparisonItem>, RegistryError> {
        let (_, items) = self
            .send::<Vec<ComparisonItem>>(
                reqwest::Method::GET,
                &format!("/v1/skills/{}/comparisons", encode(fork_id)),
                None,
            )
            .await?;
        Ok(items)
    }

    async fn deprecate(&self, id: &str, reason: &str) -> Result<SkillArtifact, RegistryError> {
        let body = serde_json::to_value(DeprecateRequest {
            reason: reason.to_owned(),
        })
        .map_err(|error| RegistryError::Invalid(error.to_string()))?;
        let (_, skill) = self
            .send::<SkillArtifact>(
                reqwest::Method::POST,
                &format!("/v1/skills/{}/deprecate", encode(id)),
                Some(body),
            )
            .await?;
        Ok(skill)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_urls_are_https_or_loopback_http_without_credentials() {
        assert_eq!(
            validate_registry_url("https://shop.example/").unwrap(),
            "https://shop.example"
        );
        assert!(validate_registry_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_registry_url("http://localhost:8080/").is_ok());
        assert!(validate_registry_url("http://shop.example").is_err());
        assert!(validate_registry_url("https://user:pw@shop.example").is_err());
        assert!(validate_registry_url("https://shop.example/?token=x").is_err());
        assert!(validate_registry_url("not a url").is_err());
    }
}
