//! The registry HTTP API. Identity is always the server-authenticated
//! principal of the bearer token; the request body never names a publisher,
//! evaluator, organization or team. Public verified and deprecated skills
//! are readable without authentication; everything else needs a token, and
//! every write needs one.
use crate::{
    auth::bearer_token,
    store::{ListFilter, Principal, RegistryStore, StoreError, Viewer},
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use s_code_skill_shop::{
    DeprecateRequest, ReceiptAccepted, SkillPublication, SkillReceiptSubmission, SkillStatus,
};
use serde::Deserialize;
use std::sync::Arc;

pub const MAX_REQUEST_BYTES: usize = 256 * 1024;

/// How the shop sets its session cookie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebSettings {
    /// Loopback development only: omit the `Secure` attribute so a browser
    /// on plain `http://127.0.0.1` can keep the session. Never accepted on
    /// a non-loopback bind.
    pub insecure_cookies: bool,
}

#[derive(Clone)]
pub struct RegistryState {
    pub store: Arc<RegistryStore>,
    pub web: WebSettings,
}

#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound,
    BadRequest(String),
    Conflict(String),
    Internal(String),
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::NotFound => Self::NotFound,
            StoreError::Forbidden => Self::Forbidden,
            StoreError::Conflict(message) => Self::Conflict(message),
            StoreError::Invalid(message) => Self::BadRequest(message),
            StoreError::Storage(message) => Self::Internal(message),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_owned()),
            Self::NotFound => (StatusCode::NOT_FOUND, "not found".to_owned()),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::Internal(message) => {
                tracing::error!(error = %message, "registry request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_owned(),
                )
            }
        };
        (status, Json(serde_json::json!({"error": message}))).into_response()
    }
}

/// The viewer for a request: an authenticated principal when a valid bearer
/// token is present, anonymous when no token is sent, and an error when a
/// token is sent but unknown, malformed or disabled.
pub async fn viewer(state: &RegistryState, headers: &HeaderMap) -> Result<Viewer, ApiError> {
    match bearer_token(headers) {
        None => Ok(Viewer::anonymous()),
        Some(Err(())) => Err(ApiError::Unauthorized),
        Some(Ok(token)) => match state.store.authenticate(&token).await? {
            Some(principal) => Ok(Viewer {
                principal: Some(principal),
            }),
            None => Err(ApiError::Unauthorized),
        },
    }
}

async fn principal(state: &RegistryState, headers: &HeaderMap) -> Result<Principal, ApiError> {
    viewer(state, headers)
        .await?
        .principal
        .ok_or(ApiError::Unauthorized)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "s-code-skill-registry",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn me(
    State(state): State<RegistryState>,
    headers: HeaderMap,
) -> Result<Json<Principal>, ApiError> {
    Ok(Json(principal(&state, &headers).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    pub status: Option<String>,
    pub q: Option<String>,
    pub task_family: Option<String>,
    pub model: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl ListQuery {
    pub fn filter(&self) -> Result<ListFilter, ApiError> {
        let status = match self.status.as_deref() {
            None | Some("") | Some("any") => None,
            Some(value) => Some(
                SkillStatus::parse(value)
                    .ok_or_else(|| ApiError::BadRequest("unknown status filter".into()))?,
            ),
        };
        for value in [&self.q, &self.task_family, &self.model] {
            if value.as_ref().is_some_and(|text| {
                text.chars().count() > 200 || text.chars().any(char::is_control)
            }) {
                return Err(ApiError::BadRequest(
                    "filters must be short plain text".into(),
                ));
            }
        }
        Ok(ListFilter {
            status,
            q: self.q.clone().filter(|q| !q.trim().is_empty()),
            task_family: self.task_family.clone().filter(|f| !f.trim().is_empty()),
            model: self.model.clone().filter(|m| !m.trim().is_empty()),
            limit: self.limit,
            offset: self.offset,
        })
    }
}

async fn list_skills(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<s_code_skill_shop::SkillArtifact>>, ApiError> {
    let viewer = viewer(&state, &headers).await?;
    Ok(Json(
        state.store.list_skills(&viewer, &query.filter()?).await?,
    ))
}

async fn get_skill(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<s_code_skill_shop::SkillArtifact>, ApiError> {
    let viewer = viewer(&state, &headers).await?;
    Ok(Json(state.store.get_skill(&viewer, &id).await?))
}

async fn list_receipts(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<s_code_skill_shop::SkillReceiptItem>>, ApiError> {
    let viewer = viewer(&state, &headers).await?;
    Ok(Json(state.store.list_receipts(&viewer, &id).await?))
}

async fn publish_skill(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Json(publication): Json<SkillPublication>,
) -> Result<(StatusCode, Json<s_code_skill_shop::SkillArtifact>), ApiError> {
    let publisher = principal(&state, &headers).await?;
    let (skill, created) = state.store.publish(&publisher, &publication).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(skill)))
}

async fn submit_receipt(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(submission): Json<SkillReceiptSubmission>,
) -> Result<(StatusCode, Json<ReceiptAccepted>), ApiError> {
    let evaluator = principal(&state, &headers).await?;
    let recorded = state
        .store
        .record_receipt(&evaluator, &id, &submission)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ReceiptAccepted {
            receipt: recorded.receipt,
            skill: recorded.skill,
            transition: recorded.transition.label(),
        }),
    ))
}

async fn deprecate_skill(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<DeprecateRequest>,
) -> Result<Json<s_code_skill_shop::SkillArtifact>, ApiError> {
    let principal = principal(&state, &headers).await?;
    Ok(Json(
        state
            .store
            .deprecate(&principal, &id, &input.reason)
            .await?,
    ))
}

pub fn api_router(state: RegistryState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/me", get(me))
        .route("/v1/skills", get(list_skills).post(publish_skill))
        .route("/v1/skills/{id}", get(get_skill))
        .route(
            "/v1/skills/{id}/evaluations",
            get(list_receipts).post(submit_receipt),
        )
        .route("/v1/skills/{id}/deprecate", post(deprecate_skill))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}
