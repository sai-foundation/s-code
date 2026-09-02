use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use opencoding_protocol::Id;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::RwLock;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("identity configuration is invalid: {0}")]
    Configuration(String),
    #[error("identity provider request failed: {0}")]
    Provider(String),
    #[error("token is invalid: {0}")]
    Token(String),
    #[error("multi-factor authentication is required")]
    MfaRequired,
}

/// A short-lived, daemon-specific capability issued by the enterprise control plane.
/// It binds every interactive request to one human, device, organization, and Team.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamGrantClaims {
    pub grant_id: String,
    pub issuer: String,
    pub audience: String,
    pub subject: String,
    pub organization_id: Id,
    pub team_id: Id,
    pub actor_id: Id,
    pub device_id: String,
    #[serde(default)]
    pub roles: BTreeSet<String>,
    pub issued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A short-lived, single-action authorization produced after an Enterprise
/// central approval. The signed token is deliberately domain-separated from
/// an interactive Team grant so it cannot be presented as a daemon session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorApprovalClaims {
    pub approval_id: Id,
    pub issuer: String,
    pub audience: String,
    pub organization_id: Id,
    pub team_id: Id,
    pub requested_by: Id,
    pub approved_by: Id,
    pub resource_type: String,
    pub resource_id: Id,
    pub action: String,
    pub idempotency_key: String,
    pub issued_at: DateTime<Utc>,
    pub not_before: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TeamGrantVerifier {
    key_id: String,
    issuer: String,
    audience: String,
    key: VerifyingKey,
    max_lifetime: Duration,
}

impl TeamGrantVerifier {
    pub fn from_base64(
        key_id: impl Into<String>,
        public_key_base64: &str,
        issuer: impl Into<String>,
        audience: impl Into<String>,
    ) -> Result<Self, IdentityError> {
        let bytes = URL_SAFE_NO_PAD.decode(public_key_base64).map_err(|_| {
            IdentityError::Configuration("Team grant public key is not base64url".into())
        })?;
        let key = VerifyingKey::from_bytes(bytes.as_slice().try_into().map_err(|_| {
            IdentityError::Configuration("Team grant public key must be 32 bytes".into())
        })?)
        .map_err(|_| IdentityError::Configuration("Team grant public key is invalid".into()))?;
        let key_id = key_id.into();
        let issuer = issuer.into();
        let audience = audience.into();
        if key_id.is_empty()
            || key_id.len() > 128
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || issuer.is_empty()
            || audience.is_empty()
        {
            return Err(IdentityError::Configuration(
                "Team grant key id must be bounded URL-safe text; issuer and audience are required"
                    .into(),
            ));
        }
        Ok(Self {
            key_id,
            issuer,
            audience,
            key,
            max_lifetime: Duration::minutes(15),
        })
    }

    pub fn verify(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<TeamGrantClaims, IdentityError> {
        let mut parts = token.split('.');
        let version = parts.next();
        let key_id = parts.next();
        let encoded_claims = parts.next();
        let encoded_signature = parts.next();
        if version != Some("v1") || key_id != Some(self.key_id.as_str()) || parts.next().is_some() {
            return Err(IdentityError::Token(
                "malformed Team grant or unknown signing key".into(),
            ));
        }
        let encoded_claims = encoded_claims
            .ok_or_else(|| IdentityError::Token("Team grant has no claims".into()))?;
        let signature_bytes = URL_SAFE_NO_PAD
            .decode(
                encoded_signature
                    .ok_or_else(|| IdentityError::Token("Team grant has no signature".into()))?,
            )
            .map_err(|_| IdentityError::Token("Team grant signature is not base64url".into()))?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| IdentityError::Token("Team grant signature is invalid".into()))?;
        let signed = format!("v1.{}.{}", self.key_id, encoded_claims);
        self.key
            .verify(signed.as_bytes(), &signature)
            .map_err(|_| IdentityError::Token("Team grant signature verification failed".into()))?;
        let claims_bytes = URL_SAFE_NO_PAD
            .decode(encoded_claims)
            .map_err(|_| IdentityError::Token("Team grant claims are not base64url".into()))?;
        let claims: TeamGrantClaims = serde_json::from_slice(&claims_bytes)
            .map_err(|_| IdentityError::Token("Team grant claims are invalid".into()))?;
        if claims.issuer != self.issuer || claims.audience != self.audience {
            return Err(IdentityError::Token(
                "Team grant issuer or audience does not match".into(),
            ));
        }
        if claims.grant_id.is_empty()
            || claims.subject.is_empty()
            || claims.device_id.is_empty()
            || claims.organization_id.0.is_empty()
            || claims.team_id.0.is_empty()
            || claims.actor_id.0.is_empty()
            || claims.roles.is_empty()
        {
            return Err(IdentityError::Token(
                "Team grant is missing required identity claims".into(),
            ));
        }
        if claims.not_before > now
            || claims.expires_at <= now
            || claims.issued_at > now + Duration::seconds(30)
        {
            return Err(IdentityError::Token(
                "Team grant is not currently valid".into(),
            ));
        }
        if claims.expires_at <= claims.issued_at
            || claims.expires_at - claims.issued_at > self.max_lifetime
        {
            return Err(IdentityError::Token(
                "Team grant lifetime exceeds 15 minutes".into(),
            ));
        }
        Ok(claims)
    }

    pub fn verify_connector_approval(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<ConnectorApprovalClaims, IdentityError> {
        let mut parts = token.split('.');
        let domain = parts.next();
        let key_id = parts.next();
        let encoded_claims = parts.next();
        let encoded_signature = parts.next();
        if domain != Some("connector-approval-v1")
            || key_id != Some(self.key_id.as_str())
            || parts.next().is_some()
        {
            return Err(IdentityError::Token(
                "malformed connector approval or unknown signing key".into(),
            ));
        }
        let encoded_claims = encoded_claims
            .ok_or_else(|| IdentityError::Token("connector approval has no claims".into()))?;
        let signature_bytes = URL_SAFE_NO_PAD
            .decode(encoded_signature.ok_or_else(|| {
                IdentityError::Token("connector approval has no signature".into())
            })?)
            .map_err(|_| {
                IdentityError::Token("connector approval signature is not base64url".into())
            })?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| IdentityError::Token("connector approval signature is invalid".into()))?;
        let signed = format!("connector-approval-v1.{}.{}", self.key_id, encoded_claims);
        self.key
            .verify(signed.as_bytes(), &signature)
            .map_err(|_| {
                IdentityError::Token("connector approval signature verification failed".into())
            })?;
        let claims_bytes = URL_SAFE_NO_PAD.decode(encoded_claims).map_err(|_| {
            IdentityError::Token("connector approval claims are not base64url".into())
        })?;
        let claims: ConnectorApprovalClaims = serde_json::from_slice(&claims_bytes)
            .map_err(|_| IdentityError::Token("connector approval claims are invalid".into()))?;
        if claims.issuer != self.issuer || claims.audience != self.audience {
            return Err(IdentityError::Token(
                "connector approval issuer or audience does not match".into(),
            ));
        }
        let bounded = |value: &str, limit: usize| {
            !value.is_empty()
                && value.len() <= limit
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "._:/@*?-".contains(character)
                })
        };
        if !bounded(&claims.approval_id.0, 256)
            || !bounded(&claims.organization_id.0, 256)
            || !bounded(&claims.team_id.0, 256)
            || !bounded(&claims.requested_by.0, 256)
            || !bounded(&claims.approved_by.0, 256)
            || claims.requested_by == claims.approved_by
            || claims.resource_type != "connector_write"
            || !bounded(&claims.resource_id.0, 256)
            || !bounded(&claims.action, 128)
            || !bounded(&claims.idempotency_key, 128)
        {
            return Err(IdentityError::Token(
                "connector approval claims are incomplete or unsafe".into(),
            ));
        }
        if claims.not_before > now
            || claims.expires_at <= now
            || claims.issued_at > now + Duration::seconds(30)
            || claims.expires_at <= claims.issued_at
            || claims.expires_at - claims.issued_at > Duration::minutes(5)
        {
            return Err(IdentityError::Token(
                "connector approval is outside its five-minute validity window".into(),
            ));
        }
        Ok(claims)
    }
}

#[derive(Clone)]
pub struct TeamGrantSigner {
    key_id: String,
    key: SigningKey,
}

impl TeamGrantSigner {
    pub fn from_base64(
        key_id: impl Into<String>,
        private_key_base64: &str,
    ) -> Result<Self, IdentityError> {
        let bytes = URL_SAFE_NO_PAD.decode(private_key_base64).map_err(|_| {
            IdentityError::Configuration("Team grant private key is not base64url".into())
        })?;
        let key = SigningKey::from_bytes(bytes.as_slice().try_into().map_err(|_| {
            IdentityError::Configuration("Team grant private key must be 32 bytes".into())
        })?);
        let key_id = key_id.into();
        if key_id.is_empty()
            || key_id.len() > 128
            || !key_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(IdentityError::Configuration(
                "Team grant key id must be bounded URL-safe text".into(),
            ));
        }
        Ok(Self { key_id, key })
    }

    pub fn sign(&self, claims: &TeamGrantClaims) -> Result<String, IdentityError> {
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims).map_err(|error| IdentityError::Token(error.to_string()))?,
        );
        let signed = format!("v1.{}.{}", self.key_id, claims);
        let signature = self.key.sign(signed.as_bytes());
        Ok(format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    pub fn sign_connector_approval(
        &self,
        claims: &ConnectorApprovalClaims,
    ) -> Result<String, IdentityError> {
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(claims).map_err(|error| IdentityError::Token(error.to_string()))?,
        );
        let signed = format!("connector-approval-v1.{}.{}", self.key_id, claims);
        let signature = self.key.sign(signed.as_bytes());
        Ok(format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }

    pub fn public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.key.verifying_key().as_bytes())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: serde_json::Value,
    pub exp: usize,
    pub iat: Option<usize>,
    pub nbf: Option<usize>,
    pub email: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub amr: Vec<String>,
    pub nonce: Option<String>,
    /// Set by a trusted federation broker when authentication originated at
    /// a non-OIDC enterprise identity provider.
    pub upstream_protocol: Option<String>,
    pub upstream_issuer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FederationMode {
    Oidc,
    SamlViaOidcBridge { upstream_issuer: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    pub principal_id: Id,
    pub subject: String,
    pub email: Option<String>,
    pub groups: BTreeSet<String>,
    pub mfa: bool,
    pub issuer: String,
}

#[derive(Clone)]
pub struct OidcVerifier {
    issuer: String,
    audience: String,
    jwks_uri: String,
    require_mfa: bool,
    federation_mode: FederationMode,
    client: reqwest::Client,
    cache: Arc<RwLock<JwksCache>>,
}

#[derive(Default)]
struct JwksCache {
    keys: Option<JwkSet>,
    fetched_at: Option<DateTime<Utc>>,
}

impl OidcVerifier {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_uri: impl Into<String>,
        require_mfa: bool,
    ) -> Result<Self, IdentityError> {
        Self::new_with_federation(
            issuer,
            audience,
            jwks_uri,
            require_mfa,
            FederationMode::Oidc,
            oidc_http_client()?,
        )
    }

    pub fn new_saml_bridge(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_uri: impl Into<String>,
        require_mfa: bool,
        upstream_issuer: impl Into<String>,
    ) -> Result<Self, IdentityError> {
        let upstream_issuer = upstream_issuer.into();
        if upstream_issuer.trim().is_empty() || upstream_issuer.len() > 2048 {
            return Err(IdentityError::Configuration(
                "SAML upstream issuer is required and must be bounded".into(),
            ));
        }
        Self::new_with_federation(
            issuer,
            audience,
            jwks_uri,
            require_mfa,
            FederationMode::SamlViaOidcBridge { upstream_issuer },
            oidc_http_client()?,
        )
    }

    fn new_with_federation(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        jwks_uri: impl Into<String>,
        require_mfa: bool,
        federation_mode: FederationMode,
        client: reqwest::Client,
    ) -> Result<Self, IdentityError> {
        let issuer = issuer.into().trim_end_matches('/').to_owned();
        let audience = audience.into();
        let jwks_uri = jwks_uri.into();
        if issuer.is_empty() || audience.is_empty() {
            return Err(IdentityError::Configuration(
                "issuer and audience are required".into(),
            ));
        }
        for endpoint in [&issuer, &jwks_uri] {
            let url = url::Url::parse(endpoint)
                .map_err(|error| IdentityError::Configuration(error.to_string()))?;
            let loopback = url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
            {
                return Err(IdentityError::Configuration(
                    "OIDC endpoints must use clean HTTPS URLs (loopback HTTP is allowed)".into(),
                ));
            }
        }
        Ok(Self {
            issuer,
            audience,
            jwks_uri,
            require_mfa,
            federation_mode,
            client,
            cache: Arc::new(RwLock::new(JwksCache::default())),
        })
    }

    pub async fn verify(&self, token: &str) -> Result<AuthenticatedPrincipal, IdentityError> {
        self.verify_with_nonce(token, None).await
    }

    pub async fn verify_with_nonce(
        &self,
        token: &str,
        expected_nonce: Option<&str>,
    ) -> Result<AuthenticatedPrincipal, IdentityError> {
        let header =
            decode_header(token).map_err(|error| IdentityError::Token(error.to_string()))?;
        if !matches!(
            header.alg,
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::ES256
                | Algorithm::ES384
                | Algorithm::EdDSA
        ) {
            return Err(IdentityError::Token(
                "symmetric JWT algorithms are forbidden".into(),
            ));
        }
        let kid = header
            .kid
            .ok_or_else(|| IdentityError::Token("JWT has no kid".into()))?;
        let mut keys = self.keys(false).await?;
        let mut jwk = keys.find(&kid);
        if jwk.is_none() {
            keys = self.keys(true).await?;
            jwk = keys.find(&kid);
        }
        let jwk = jwk.ok_or_else(|| IdentityError::Token("signing key was not found".into()))?;
        let key =
            DecodingKey::from_jwk(jwk).map_err(|error| IdentityError::Token(error.to_string()))?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.leeway = 60;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.required_spec_claims =
            HashSet::from(["exp".into(), "iss".into(), "sub".into(), "aud".into()]);
        let claims = decode::<TokenClaims>(token, &key, &validation)
            .map_err(|error| IdentityError::Token(error.to_string()))?
            .claims;
        self.validate_federation_claims(&claims)?;
        if let Some(expected) = expected_nonce
            && claims.nonce.as_deref() != Some(expected)
        {
            return Err(IdentityError::Token("OIDC nonce does not match".into()));
        }
        let mfa = claims
            .amr
            .iter()
            .any(|method| matches!(method.as_str(), "mfa" | "otp" | "hwk" | "fido" | "webauthn"));
        if self.require_mfa && !mfa {
            return Err(IdentityError::MfaRequired);
        }
        Ok(AuthenticatedPrincipal {
            principal_id: Id(format!("{}:{}", self.principal_scheme(), claims.sub)),
            subject: claims.sub,
            email: claims.email,
            groups: claims.groups.into_iter().collect(),
            mfa,
            issuer: claims.iss,
        })
    }

    fn validate_federation_claims(&self, claims: &TokenClaims) -> Result<(), IdentityError> {
        match &self.federation_mode {
            FederationMode::Oidc => Ok(()),
            FederationMode::SamlViaOidcBridge { upstream_issuer } => {
                if claims.upstream_protocol.as_deref() != Some("saml") {
                    return Err(IdentityError::Token(
                        "federation token is not bound to a SAML authentication".into(),
                    ));
                }
                if claims.upstream_issuer.as_deref() != Some(upstream_issuer.as_str()) {
                    return Err(IdentityError::Token(
                        "SAML upstream issuer does not match the configured entity ID".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn principal_scheme(&self) -> &'static str {
        match self.federation_mode {
            FederationMode::Oidc => "oidc",
            FederationMode::SamlViaOidcBridge { .. } => "saml",
        }
    }

    async fn keys(&self, force: bool) -> Result<JwkSet, IdentityError> {
        let stale = {
            let cache = self.cache.read().await;
            force
                || cache.keys.is_none()
                || cache
                    .fetched_at
                    .is_none_or(|time| Utc::now() - time > Duration::minutes(15))
        };
        if stale {
            let response = self
                .client
                .get(&self.jwks_uri)
                .send()
                .await
                .map_err(|error| IdentityError::Provider(error.to_string()))?;
            if !response.status().is_success() {
                return Err(IdentityError::Provider(response.status().to_string()));
            }
            let keys: JwkSet = bounded_oidc_json(response, 64 * 1024, "JWKS").await?;
            if keys.keys.is_empty() || keys.keys.len() > 128 {
                return Err(IdentityError::Provider(
                    "JWKS must contain between 1 and 128 keys".into(),
                ));
            }
            let mut cache = self.cache.write().await;
            cache.keys = Some(keys);
            cache.fetched_at = Some(Utc::now());
        }
        self.cache
            .read()
            .await
            .keys
            .clone()
            .ok_or_else(|| IdentityError::Provider("JWKS cache is empty".into()))
    }
}

fn oidc_http_client() -> Result<reqwest::Client, IdentityError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| IdentityError::Configuration(error.to_string()))
}

async fn bounded_oidc_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<T, IdentityError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(IdentityError::Provider(format!(
            "{label} response exceeds {limit} bytes"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| IdentityError::Provider(error.to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(IdentityError::Provider(format!(
                "{label} response exceeds {limit} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|error| IdentityError::Provider(error.to_string()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    OrganizationAdmin,
    TeamLead,
    Developer,
    Auditor,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    ManageOrganization,
    ManagePeople,
    ManageDevices,
    ManageModels,
    ManagePolicies,
    ManageIntegrations,
    ManageExtensions,
    ReadExtensions,
    ManageOwnership,
    ManageGoals,
    ManageQueue,
    ManageKnowledge,
    ManageCapacity,
    ManageBudget,
    ExecuteAgent,
    ApproveHighRisk,
    ReadDashboard,
    ReadAudit,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Membership {
    pub organization_id: Id,
    pub team_id: Option<Id>,
    pub principal_id: Id,
    pub role: Role,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct AuthorizationRequest {
    pub organization_id: Id,
    pub team_id: Option<Id>,
    pub principal: AuthenticatedPrincipal,
    pub permission: Permission,
    pub device_compliant: bool,
    pub risk_level: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    pub reason: String,
    pub role: Option<Role>,
}

pub fn authorize(
    request: &AuthorizationRequest,
    memberships: &[Membership],
) -> AuthorizationDecision {
    if !request.device_compliant && request.permission != Permission::ReadDashboard {
        return denied("device is not compliant");
    }
    if request.risk_level == "high"
        && !request.principal.mfa
        && request.permission != Permission::ReadDashboard
    {
        return denied("MFA is required for high-risk access");
    }
    let now = Utc::now();
    let membership = memberships.iter().find(|membership| {
        membership.organization_id == request.organization_id
            && membership.principal_id == request.principal.principal_id
            && membership.valid_from <= now
            && membership.valid_until.is_none_or(|until| until > now)
            && (membership.team_id.is_none() || membership.team_id == request.team_id)
    });
    let Some(membership) = membership else {
        return denied("no active organization/team membership");
    };
    let allowed = role_permissions(&membership.role).contains(&request.permission);
    AuthorizationDecision {
        allowed,
        reason: if allowed {
            "role grants permission".into()
        } else {
            "role does not grant permission".into()
        },
        role: Some(membership.role.clone()),
    }
}

pub fn role_permissions(role: &Role) -> BTreeSet<Permission> {
    use Permission::*;
    match role {
        Role::OrganizationAdmin => BTreeSet::from([
            ManageOrganization,
            ManagePeople,
            ManageDevices,
            ManageModels,
            ManagePolicies,
            ManageIntegrations,
            ManageExtensions,
            ReadExtensions,
            ManageOwnership,
            ManageGoals,
            ManageQueue,
            ManageKnowledge,
            ManageCapacity,
            ManageBudget,
            ExecuteAgent,
            ApproveHighRisk,
            ReadDashboard,
            ReadAudit,
        ]),
        Role::TeamLead => BTreeSet::from([
            ManageExtensions,
            ReadExtensions,
            ManageOwnership,
            ManageGoals,
            ManageQueue,
            ManageKnowledge,
            ManageCapacity,
            ManageBudget,
            ExecuteAgent,
            ApproveHighRisk,
            ReadDashboard,
            ReadAudit,
        ]),
        Role::Developer => BTreeSet::from([ExecuteAgent, ReadDashboard, ReadExtensions]),
        Role::Auditor => BTreeSet::from([ReadDashboard, ReadAudit, ReadExtensions]),
        Role::Agent => BTreeSet::from([ExecuteAgent, ReadExtensions]),
    }
}

pub fn named_roles_grant(roles: &BTreeSet<String>, permission: &Permission) -> bool {
    roles.iter().any(|role| {
        let role = match role.as_str() {
            "organization_admin" => Role::OrganizationAdmin,
            "team_lead" => Role::TeamLead,
            "developer" => Role::Developer,
            "auditor" => Role::Auditor,
            "agent" => Role::Agent,
            _ => return false,
        };
        role_permissions(&role).contains(permission)
    })
}

fn denied(reason: &str) -> AuthorizationDecision {
    AuthorizationDecision {
        allowed: false,
        reason: reason.into(),
        role: None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScimUser {
    pub id: String,
    pub user_name: String,
    pub active: bool,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScimGroup {
    pub id: String,
    pub display_name: String,
    pub members: BTreeSet<String>,
}

#[derive(Default)]
pub struct DirectoryState {
    pub users: std::collections::BTreeMap<String, ScimUser>,
    pub groups: std::collections::BTreeMap<String, ScimGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScimSyncResult {
    pub upserted_users: usize,
    pub deactivated_users: usize,
    pub upserted_groups: usize,
}

impl DirectoryState {
    pub fn apply_authoritative_snapshot(
        &mut self,
        users: Vec<ScimUser>,
        groups: Vec<ScimGroup>,
    ) -> ScimSyncResult {
        let incoming: BTreeSet<_> = users.iter().map(|user| user.id.clone()).collect();
        let mut deactivated = 0;
        for user in self.users.values_mut() {
            if user.active && !incoming.contains(&user.id) {
                user.active = false;
                deactivated += 1;
            }
        }
        let upserted_users = users.len();
        for user in users {
            self.users.insert(user.id.clone(), user);
        }
        let upserted_groups = groups.len();
        self.groups = groups
            .into_iter()
            .map(|group| (group.id.clone(), group))
            .collect();
        ScimSyncResult {
            upserted_users,
            deactivated_users: deactivated,
            upserted_groups,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(mfa: bool) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            principal_id: Id("user".into()),
            subject: "subject".into(),
            email: None,
            groups: BTreeSet::new(),
            mfa,
            issuer: "https://idp.example".into(),
        }
    }

    #[test]
    fn team_lead_can_manage_queue_but_developer_cannot_manage_policy() {
        let membership = Membership {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            principal_id: Id("user".into()),
            role: Role::TeamLead,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: None,
        };
        let request = AuthorizationRequest {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            principal: principal(true),
            permission: Permission::ManageQueue,
            device_compliant: true,
            risk_level: "normal".into(),
        };
        assert!(authorize(&request, std::slice::from_ref(&membership)).allowed);
        let developer = Membership {
            role: Role::Developer,
            ..membership
        };
        let policy = AuthorizationRequest {
            permission: Permission::ManagePolicies,
            ..request
        };
        assert!(!authorize(&policy, &[developer]).allowed);
    }

    #[test]
    fn high_risk_and_noncompliant_devices_fail_closed() {
        let membership = Membership {
            organization_id: Id("org".into()),
            team_id: None,
            principal_id: Id("user".into()),
            role: Role::OrganizationAdmin,
            valid_from: Utc::now() - Duration::minutes(1),
            valid_until: None,
        };
        let request = AuthorizationRequest {
            organization_id: Id("org".into()),
            team_id: Some(Id("team".into())),
            principal: principal(false),
            permission: Permission::ManagePolicies,
            device_compliant: true,
            risk_level: "high".into(),
        };
        assert!(!authorize(&request, std::slice::from_ref(&membership)).allowed);
        let noncompliant = AuthorizationRequest {
            principal: principal(true),
            device_compliant: false,
            ..request
        };
        assert!(!authorize(&noncompliant, &[membership]).allowed);
    }

    #[test]
    fn scim_snapshot_deactivates_removed_users() {
        let mut directory = DirectoryState::default();
        directory.users.insert(
            "old".into(),
            ScimUser {
                id: "old".into(),
                user_name: "old@example.com".into(),
                active: true,
                display_name: None,
            },
        );
        let result = directory.apply_authoritative_snapshot(
            vec![ScimUser {
                id: "new".into(),
                user_name: "new@example.com".into(),
                active: true,
                display_name: None,
            }],
            vec![ScimGroup {
                id: "group".into(),
                display_name: "Developers".into(),
                members: BTreeSet::from(["new".into()]),
            }],
        );
        assert_eq!(result.deactivated_users, 1);
        assert!(!directory.users["old"].active);
        assert!(directory.users["new"].active);
    }

    #[test]
    fn oidc_configuration_rejects_insecure_remote_endpoints() {
        assert!(
            OidcVerifier::new("http://idp.example", "aud", "http://idp.example/jwks", true)
                .is_err()
        );
        assert!(
            OidcVerifier::new("http://127.0.0.1:1", "aud", "http://127.0.0.1:1/jwks", true).is_ok()
        );
        for endpoint in [
            "https://user@idp.example/jwks",
            "https://idp.example/jwks?tenant=secret",
            "https://idp.example/jwks#fragment",
            "ftp://127.0.0.1/jwks",
        ] {
            assert!(
                OidcVerifier::new("https://idp.example", "aud", endpoint, true).is_err(),
                "accepted unsafe endpoint {endpoint}"
            );
        }
    }

    async fn serve_one_http_response(response: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn oidc_jwks_rejects_redirects_and_oversized_chunked_bodies() {
        let redirect_base = serve_one_http_response(
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        )
        .await;
        let redirect = OidcVerifier::new(
            &redirect_base,
            "aud",
            format!("{redirect_base}/jwks"),
            false,
        )
        .unwrap()
        .keys(false)
        .await
        .unwrap_err();
        assert!(redirect.to_string().contains("302"));

        let oversized = vec![b'x'; 64 * 1024 + 1];
        let mut response = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
            oversized.len()
        )
        .into_bytes();
        response.extend_from_slice(&oversized);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        let oversized_base = serve_one_http_response(response).await;
        let error = OidcVerifier::new(
            &oversized_base,
            "aud",
            format!("{oversized_base}/jwks"),
            false,
        )
        .unwrap()
        .keys(false)
        .await
        .unwrap_err();
        assert!(error.to_string().contains("exceeds 65536 bytes"));
    }

    #[test]
    fn saml_bridge_requires_protocol_and_exact_upstream_issuer_binding() {
        let verifier = OidcVerifier::new_saml_bridge(
            "http://127.0.0.1:1",
            "aud",
            "http://127.0.0.1:1/jwks",
            true,
            "https://idp.example/saml/metadata",
        )
        .unwrap();
        let mut claims = TokenClaims {
            iss: "http://127.0.0.1:1".into(),
            sub: "alice".into(),
            aud: serde_json::json!("aud"),
            exp: (Utc::now() + Duration::minutes(5)).timestamp() as usize,
            iat: None,
            nbf: None,
            email: None,
            groups: vec![],
            amr: vec!["mfa".into()],
            nonce: None,
            upstream_protocol: Some("saml".into()),
            upstream_issuer: Some("https://idp.example/saml/metadata".into()),
        };
        assert!(verifier.validate_federation_claims(&claims).is_ok());
        assert_eq!(verifier.principal_scheme(), "saml");

        claims.upstream_issuer = Some("https://attacker.example/metadata".into());
        assert!(verifier.validate_federation_claims(&claims).is_err());
        claims.upstream_issuer = Some("https://idp.example/saml/metadata".into());
        claims.upstream_protocol = Some("oidc".into());
        assert!(verifier.validate_federation_claims(&claims).is_err());
        assert!(
            OidcVerifier::new_saml_bridge(
                "http://127.0.0.1:1",
                "aud",
                "http://127.0.0.1:1/jwks",
                true,
                "",
            )
            .is_err()
        );
    }

    #[test]
    fn team_grants_are_signed_scoped_and_short_lived() {
        let private_key = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        let signer = TeamGrantSigner::from_base64("key-1", &private_key).unwrap();
        let verifier = TeamGrantVerifier::from_base64(
            "key-1",
            &signer.public_key_base64(),
            "control-plane",
            "daemon",
        )
        .unwrap();
        let now = Utc::now();
        let claims = TeamGrantClaims {
            grant_id: "grant-1".into(),
            issuer: "control-plane".into(),
            audience: "daemon".into(),
            subject: "oidc:alice".into(),
            organization_id: Id("org-a".into()),
            team_id: Id("team-a".into()),
            actor_id: Id("alice".into()),
            device_id: "device-a".into(),
            roles: BTreeSet::from(["developer".into()]),
            issued_at: now,
            not_before: now - Duration::seconds(1),
            expires_at: now + Duration::minutes(5),
        };
        let token = signer.sign(&claims).unwrap();
        assert_eq!(verifier.verify(&token, now).unwrap(), claims);

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(
            verifier
                .verify(std::str::from_utf8(&tampered).unwrap(), now)
                .is_err()
        );

        let mut overlong = claims;
        overlong.expires_at = now + Duration::minutes(16);
        assert!(
            verifier
                .verify(&signer.sign(&overlong).unwrap(), now)
                .is_err()
        );
    }

    #[test]
    fn connector_approvals_are_domain_separated_two_person_and_short_lived() {
        let signer =
            TeamGrantSigner::from_base64("key-1", &URL_SAFE_NO_PAD.encode([8_u8; 32])).unwrap();
        let verifier = TeamGrantVerifier::from_base64(
            "key-1",
            &signer.public_key_base64(),
            "control-plane",
            "daemon",
        )
        .unwrap();
        let now = Utc::now();
        let claims = ConnectorApprovalClaims {
            approval_id: Id("approval-1".into()),
            issuer: "control-plane".into(),
            audience: "daemon".into(),
            organization_id: Id("org-a".into()),
            team_id: Id("team-a".into()),
            requested_by: Id("alice".into()),
            approved_by: Id("bob".into()),
            resource_type: "connector_write".into(),
            resource_id: Id("0123456789abcdef0123456789abcdef".into()),
            action: "servicenow.append_work_note".into(),
            idempotency_key: "snow-write-0001".into(),
            issued_at: now,
            not_before: now - Duration::seconds(1),
            expires_at: now + Duration::minutes(5),
        };
        let token = signer.sign_connector_approval(&claims).unwrap();
        assert_eq!(
            verifier.verify_connector_approval(&token, now).unwrap(),
            claims
        );
        assert!(verifier.verify(&token, now).is_err());

        let mut self_approved = claims.clone();
        self_approved.approved_by = self_approved.requested_by.clone();
        assert!(
            verifier
                .verify_connector_approval(
                    &signer.sign_connector_approval(&self_approved).unwrap(),
                    now,
                )
                .is_err()
        );
        let mut overlong = claims;
        overlong.expires_at = now + Duration::minutes(6);
        assert!(
            verifier
                .verify_connector_approval(
                    &signer.sign_connector_approval(&overlong).unwrap(),
                    now,
                )
                .is_err()
        );
    }
}
