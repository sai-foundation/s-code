use super::*;
use s_code_model_gateway::onboarding::{self, Connection, SetupRequest};

fn local_setup<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
) -> Result<&'a s_code_model_gateway::onboarding::LocalProvider, ApiError> {
    authorize(state, headers)?.ensure_development()?;
    state.local_provider.as_deref().ok_or(ApiError::Forbidden)
}

pub(super) async fn catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let provider = local_setup(&state, &headers)?;
    Ok(Json(
        serde_json::json!({ "providers": onboarding::presets(), "configured": provider.configured(), "credentials_available": provider.credentials_available(), "needs_setup": provider.needs_setup && s_code_config::onboarding::read(&provider.path).map_err(ApiError::BadRequest)?.is_none() }),
    ))
}
pub(super) async fn promotions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    local_setup(&state, &headers)?;
    Ok(Json(
        serde_json::json!({ "promotions": onboarding::promotions().await }),
    ))
}
pub(super) async fn models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(connection): Json<Connection>,
) -> Result<Json<serde_json::Value>, ApiError> {
    local_setup(&state, &headers)?;
    let _guard = state
        .provider_setup_lock
        .try_lock()
        .map_err(|_| ApiError::Conflict("Another provider connection check is running".into()))?;
    let models = onboarding::discover(&connection)
        .await
        .map_err(ApiError::BadRequest)?;
    Ok(Json(serde_json::json!({ "models": models })))
}
pub(super) async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SetupRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let provider = local_setup(&state, &headers)?;
    let _guard = state
        .provider_setup_lock
        .try_lock()
        .map_err(|_| ApiError::Conflict("Another provider connection check is running".into()))?;
    onboarding::save_setup(&provider.path, &request)
        .await
        .map_err(ApiError::BadRequest)?;
    let mut settings = state.store.get_settings().await?;
    settings.default_model = request.model.clone();
    state.store.put_settings(&settings).await?;
    Ok(Json(
        serde_json::json!({ "model": request.model, "configured": true }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use tower::ServiceExt;
    #[cfg(unix)]
    #[tokio::test]
    async fn setup_requires_local_auth_and_never_returns_a_saved_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        let store = Store::in_memory().await.unwrap();
        let state =
            AppState::new("test-session", store, 0).with_local_provider_setup(path.clone(), true);
        let service = app(state);
        let anonymous = service
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/provider-setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        s_code_config::onboarding::save(
            &path,
            &s_code_config::onboarding::SavedProvider {
                provider: "openai_compatible".into(),
                base_url: "https://api.sai.foundation/v1".into(),
                api_key: "unit-private-provider-key".into(),
                model: "chat-model".into(),
            },
        )
        .unwrap();
        let response = service
            .oneshot(
                Request::builder()
                    .uri("/v1/provider-setup")
                    .header(header::AUTHORIZATION, "Bearer test-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 32768).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("unit-private-provider-key"));
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["providers"][0]["id"], "sai");
        assert_eq!(body["configured"], true);
    }
    #[tokio::test]
    async fn setup_is_disabled_without_local_configuration_and_health_starts_unconfigured() {
        let store = Store::in_memory().await.unwrap();
        let state = AppState::new("test-session", store, 0);
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/v1/provider-setup")
                    .header(header::AUTHORIZATION, "Bearer test-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let directory = tempfile::tempdir().unwrap();
        let state = state.with_local_provider_setup(directory.path().join("provider.json"), true);
        assert!(!state.provider_configured());
        assert!(
            state
                .start_durable_worker("test")
                .map(|worker| worker.abort())
                .is_some()
        );
    }
}
