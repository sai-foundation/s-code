//! `s-code-skill-registry`: the authoritative online shared skill registry.
//!
//! A separately runnable service, never a mode of the local daemon. It holds
//! server-authenticated principals, shared skills with team or public
//! visibility, immutable population receipts and a bounded audit log, and it
//! applies exactly the domain rules of `s-code-skill-shop` that the local
//! shop applies: sanitized publication, receipt validation, the protocol-1
//! arm gate and the deterministic verification gate, all inside one SQLite
//! write transaction per receipt.
//!
//! The service speaks plain HTTP and binds loopback by default. For any
//! deployment beyond one machine it must sit behind a TLS-terminating
//! reverse proxy; plain public HTTP is not secure and is refused unless the
//! operator explicitly acknowledges a proxy in front of it.
pub mod api;
pub mod auth;
pub mod shop;
pub mod store;

pub use api::{ApiError, RegistryState, WebSettings, api_router};
pub use shop::{TRUST_NOTICE, shop_router};
pub use store::{ListFilter, Principal, RegistryStore, StoreError, Viewer};

use axum::Router;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tower_http::trace::TraceLayer;

pub const DEFAULT_BIND: &str = "127.0.0.1:18790";
pub const DEFAULT_DATA_DIR: &str = "skill-registry-data";
pub const CONNECTION_FILE: &str = "registry.json";
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct RegistryConfig {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    /// Set only when a reverse proxy terminates TLS in front of a
    /// non-loopback bind.
    pub behind_tls_proxy: bool,
    /// Loopback development only: shop session cookies without `Secure`.
    pub insecure_cookies: bool,
}

impl RegistryConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("S_CODE_SKILL_REGISTRY_BIND")
            .unwrap_or_else(|_| DEFAULT_BIND.into())
            .parse::<SocketAddr>()?;
        let data_dir = std::env::var("S_CODE_SKILL_REGISTRY_DATA_DIR")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
        let behind_tls_proxy = matches!(
            std::env::var("S_CODE_SKILL_REGISTRY_BEHIND_TLS_PROXY").as_deref(),
            Ok("1") | Ok("true")
        );
        let insecure_cookies = matches!(
            std::env::var("S_CODE_SKILL_REGISTRY_INSECURE_COOKIES").as_deref(),
            Ok("1") | Ok("true")
        );
        let config = Self {
            bind,
            data_dir,
            behind_tls_proxy,
            insecure_cookies,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.bind.ip().is_loopback() && !self.behind_tls_proxy {
            anyhow::bail!(
                "binding {} outside loopback requires a TLS-terminating reverse proxy; set S_CODE_SKILL_REGISTRY_BEHIND_TLS_PROXY=1 only when one is in front of this service",
                self.bind
            );
        }
        if self.insecure_cookies && (!self.bind.ip().is_loopback() || self.behind_tls_proxy) {
            anyhow::bail!(
                "S_CODE_SKILL_REGISTRY_INSECURE_COOKIES is a loopback development setting; it is refused for bind {} or behind a TLS proxy",
                self.bind
            );
        }
        Ok(())
    }

    pub fn web_settings(&self) -> WebSettings {
        WebSettings {
            insecure_cookies: self.insecure_cookies,
        }
    }
}

/// The complete application with deployment cookie settings: JSON API plus
/// the browsable shop.
pub fn app(store: Arc<RegistryStore>) -> Router {
    app_with(store, WebSettings::default())
}

pub fn app_with(store: Arc<RegistryStore>, web: WebSettings) -> Router {
    let state = RegistryState { store, web };
    api_router(state.clone())
        .merge(shop_router(state))
        // Request spans name the method and path only: a query string is
        // never logged, so a misdirected `?token=` cannot reach the log.
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
            tracing::info_span!("request", method = %request.method(), path = %request.uri().path())
        }))
}

/// A running registry on an ephemeral or configured port, for the binary,
/// tests and the population experiment. Dropping the handle stops it.
pub struct RunningRegistry {
    pub address: SocketAddr,
    pub store: Arc<RegistryStore>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RunningRegistry {
    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(SHUTDOWN_GRACE, task).await;
        }
    }
}

impl Drop for RunningRegistry {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Bind and serve until `stop` is called or the handle is dropped.
pub async fn start(bind: SocketAddr, store: Arc<RegistryStore>) -> anyhow::Result<RunningRegistry> {
    start_with(bind, store, WebSettings::default()).await
}

pub async fn start_with(
    bind: SocketAddr,
    store: Arc<RegistryStore>,
    web: WebSettings,
) -> anyhow::Result<RunningRegistry> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let router = app_with(store.clone(), web);
    let task = tokio::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let _ = receiver.await;
        });
        if let Err(error) = server.await {
            tracing::error!(error = %error, "skill registry server stopped with an error");
        }
    });
    Ok(RunningRegistry {
        address,
        store,
        shutdown: Some(sender),
        task: Some(task),
    })
}

/// An in-memory registry on an ephemeral loopback port, for tests.
pub async fn start_ephemeral() -> anyhow::Result<RunningRegistry> {
    let store = Arc::new(RegistryStore::in_memory().await?);
    start("127.0.0.1:0".parse()?, store).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use s_code_skill_shop::{
        ChallengeItem, ComparisonAccepted, Lineage, SKILL_SANITIZATION_VERSION, SkillArtifact,
        SkillProvenance, SkillPublication, SkillReceiptSubmission, SkillStatus, SkillVisibility,
        skill_content_digest,
    };
    use tower::ServiceExt;

    const LESSON: &str = "When a command-line tool must fail on malformed input, return a distinct non-zero exit status and write one diagnostic line to standard error.";
    const APPLICABILITY: &str = "Tools whose callers rely on exit status and stderr diagnostics.";

    async fn registry() -> (Arc<RegistryStore>, Router) {
        let store = Arc::new(RegistryStore::in_memory().await.unwrap());
        (store.clone(), app(store))
    }

    async fn principal(store: &RegistryStore, name: &str, team: &str) -> (Principal, String) {
        store.create_principal(name, "org", team).await.unwrap()
    }

    fn request(
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        match body {
            Some(body) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }

    trait IntoStatusAndBody {
        fn into_status_and_body(self) -> (StatusCode, serde_json::Value);
    }

    impl IntoStatusAndBody for (StatusCode, serde_json::Value, String) {
        fn into_status_and_body(self) -> (StatusCode, serde_json::Value) {
            (self.0, self.1)
        }
    }

    async fn send(
        router: &Router,
        request: Request<Body>,
    ) -> (StatusCode, serde_json::Value, String) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            text,
        )
    }

    fn publication(lesson: &str, visibility: SkillVisibility) -> serde_json::Value {
        serde_json::json!({
            "lesson": lesson,
            "applicability": APPLICABILITY,
            "content_digest": skill_content_digest(lesson, APPLICABILITY, SKILL_SANITIZATION_VERSION),
            "sanitization_version": SKILL_SANITIZATION_VERSION,
            "visibility": visibility,
            "provenance": {"source_kind": "distilled", "task_family": "cli-error-contract"},
        })
    }

    fn outcome(id: &str, attempts: u32, passes: u32, input: u64) -> serde_json::Value {
        let comparable = passes;
        serde_json::json!({
            "track": "project", "id": id, "attempts": attempts, "passes": passes,
            "comparable_successes": comparable,
            "median_input_units": if comparable > 0 { Some(input) } else { None },
            "median_output_units": if comparable > 0 { Some(50) } else { None },
            "median_total_units": if comparable > 0 { Some(input + 50) } else { None },
            "median_model_calls": if comparable > 0 { Some(4) } else { None },
            "median_tool_calls": if comparable > 0 { Some(6) } else { None },
            "median_wall_seconds": if comparable > 0 { Some(12.5) } else { None },
        })
    }

    fn receipt(
        skill: &serde_json::Value,
        baseline: u32,
        candidate: u32,
        safety: &str,
        version: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "skill_id": skill["id"],
            "content_digest": skill["content_digest"],
            "protocol_version": 1,
            "task_family": "cli-error-contract",
            "held_out_tasks": [{"track": "project", "id": "service-config-checker", "protected_sha256": "cd".repeat(32)}],
            "catalog_revision": "catalog-1",
            "s_code_revision": "rev-1",
            "provider": "openai-compatible",
            "model": "model-x",
            "repeats": 5,
            "baseline": [outcome("service-config-checker", 5, baseline, 1000)],
            "candidate": [outcome("service-config-checker", 5, candidate, 900)],
            "safety": {"verdict": safety, "candidate_retrieved_only_skill": safety == "clean", "harmful_rule_absent_from_requests": safety == "clean"},
            "artifact_references": ["runs/x"],
            "evaluator": {"name": "s-code-skill-evaluator", "version": version},
        })
    }

    #[tokio::test]
    async fn publication_is_authenticated_idempotent_and_never_trusts_the_body_identity() {
        let (store, router) = registry().await;
        let (alice, alice_token) = principal(&store, "Alice", "team").await;
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills",
                    None,
                    Some(publication(LESSON, SkillVisibility::Team))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills",
                    Some("skr_unknown"),
                    Some(publication(LESSON, SkillVisibility::Team))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills",
                    Some("not-a-token"),
                    Some(publication(LESSON, SkillVisibility::Team))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let mut forged = publication(LESSON, SkillVisibility::Team);
        forged["publisher"] = serde_json::json!({"id": "mallory"});
        assert_eq!(
            send(
                &router,
                request("POST", "/v1/skills", Some(&alice_token), Some(forged))
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY,
            "forged publisher field is refused"
        );
        let mut leaky = publication(LESSON, SkillVisibility::Team);
        leaky["lesson"] = serde_json::json!("Check /home/alice/project/config.toml first.");
        leaky["content_digest"] = serde_json::json!(skill_content_digest(
            "Check /home/alice/project/config.toml first.",
            APPLICABILITY,
            SKILL_SANITIZATION_VERSION
        ));
        assert_eq!(
            send(
                &router,
                request("POST", "/v1/skills", Some(&alice_token), Some(leaky))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "the registry re-sanitizes"
        );
        let (status, skill, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Team)),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{skill}");
        assert_eq!(skill["publisher"]["id"], serde_json::json!(alice.id));
        assert_eq!(
            skill["shared_scope"],
            serde_json::json!({"organization_id": "org", "team_id": "team"})
        );
        assert_eq!(skill["status"], "candidate");
        let (status, again, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Team)),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "retry is idempotent");
        assert_eq!(again["id"], skill["id"]);
        let (status, me, _) =
            send(&router, request("GET", "/v1/me", Some(&alice_token), None)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["id"], serde_json::json!(alice.id));
        assert!(me.get("token_hash").is_none() && !me.to_string().contains(&alice_token));
        // Disabled principals are refused; nothing in the audit log carries the token.
        store.disable_principal(&alice.id).await.unwrap();
        assert_eq!(
            send(&router, request("GET", "/v1/me", Some(&alice_token), None))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        let events = store.events(None).await.unwrap();
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains(&alice_token)
        );
        assert!(
            !serde_json::to_string(&store.list_principals().await.unwrap())
                .unwrap()
                .contains(&alice_token)
        );
    }

    #[tokio::test]
    async fn visibility_rules_hide_candidates_and_team_skills_from_outsiders() {
        let (store, router) = registry().await;
        let (_alice, alice_token) = principal(&store, "Alice", "team").await;
        let (_bob, bob_token) = principal(&store, "Bob", "team").await;
        let (_xavier, xavier_token) = principal(&store, "Xavier", "other-team").await;
        let (_, team_skill, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Team)),
            ),
        )
        .await;
        let public_lesson =
            "Prefer explicit exit codes over printed error prose when scripts are composed.";
        let (_, public_skill, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(public_lesson, SkillVisibility::Public)),
            ),
        )
        .await;
        let team_id = team_skill["id"].as_str().unwrap();
        let public_id = public_skill["id"].as_str().unwrap();
        // Candidates: team members only, whatever the visibility; never anonymous, never cross-team.
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}"),
                    Some(&bob_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{public_id}"),
                    Some(&bob_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::OK
        );
        for id in [team_id, public_id] {
            assert_eq!(
                send(
                    &router,
                    request("GET", &format!("/v1/skills/{id}"), None, None)
                )
                .await
                .0,
                StatusCode::NOT_FOUND,
                "anonymous never sees a candidate"
            );
            assert_eq!(
                send(
                    &router,
                    request(
                        "GET",
                        &format!("/v1/skills/{id}"),
                        Some(&xavier_token),
                        None
                    )
                )
                .await
                .0,
                StatusCode::NOT_FOUND,
                "another team never sees a candidate"
            );
            assert_eq!(
                send(
                    &router,
                    request(
                        "GET",
                        &format!("/v1/skills/{id}/evaluations"),
                        Some(&xavier_token),
                        None
                    )
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
        }
        assert!(
            send(&router, request("GET", "/v1/skills?status=any", None, None))
                .await
                .1
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            send(
                &router,
                request("GET", "/v1/skills?status=any", Some(&xavier_token), None)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            send(
                &router,
                request("GET", "/v1/skills?status=any", Some(&bob_token), None)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            2
        );
        // Verify both through two independent team receipts.
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        for (skill, id) in [(&team_skill, team_id), (&public_skill, public_id)] {
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{id}/evaluations"),
                        Some(&bob_token),
                        Some(receipt(skill, 3, 4, "clean", "1"))
                    )
                )
                .await
                .0,
                StatusCode::CREATED
            );
            let (status, accepted, _) = send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&carol_token),
                    Some(receipt(skill, 3, 4, "clean", "1")),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{accepted}");
            assert_eq!(accepted["transition"], "verified");
        }
        // Verified + team: team only. Verified + public: anyone, including anonymous and other teams.
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{team_id}"), None, None)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}"),
                    Some(&xavier_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let (status, anonymous_view, _) = send(
            &router,
            request("GET", &format!("/v1/skills/{public_id}"), None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(anonymous_view["status"], "verified");
        assert_eq!(anonymous_view["summary"]["independent_evaluators"], 2);
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{public_id}"),
                    Some(&xavier_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{public_id}/evaluations"),
                    None,
                    None
                )
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            2
        );
        // Listing and search respect visibility and pagination.
        let listed = send(&router, request("GET", "/v1/skills", None, None))
            .await
            .1;
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], public_skill["id"]);
        assert_eq!(
            send(
                &router,
                request("GET", "/v1/skills?q=exit+codes", None, None)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            send(
                &router,
                request("GET", "/v1/skills?q=standard+error", None, None)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            0,
            "team skill text never leaks through search"
        );
        assert_eq!(
            send(
                &router,
                request("GET", "/v1/skills?limit=1&offset=1", Some(&bob_token), None)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    "/v1/skills?task_family=cli-error-contract",
                    Some(&bob_token),
                    None
                )
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            2
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    "/v1/skills?model=other-model",
                    Some(&bob_token),
                    None
                )
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            0
        );
        // Outsiders cannot write to a team skill; the publisher cannot deprecate another team's skill.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{team_id}/evaluations"),
                    Some(&xavier_token),
                    Some(receipt(&team_skill, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/deprecate"),
                    Some(&xavier_token),
                    Some(serde_json::json!({"reason": "mine"}))
                )
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/deprecate"),
                    None,
                    Some(serde_json::json!({"reason": "mine"}))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn receipts_are_immutable_independent_and_verify_once_under_concurrency() {
        let (store, router) = registry().await;
        let (alice, alice_token) = principal(&store, "Alice", "team").await;
        let (_, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let (_, skill, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Team)),
            ),
        )
        .await;
        let id = skill["id"].as_str().unwrap().to_owned();
        for field in [
            "eligible",
            "verified",
            "passed_gate",
            "status",
            "independent",
            "evaluator_principal_id",
        ] {
            let mut forged = receipt(&skill, 3, 4, "clean", "1");
            forged[field] = serde_json::json!(true);
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{id}/evaluations"),
                        Some(&bob_token),
                        Some(forged)
                    )
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
        }
        let mut wrong_digest = receipt(&skill, 3, 4, "clean", "1");
        wrong_digest["content_digest"] = serde_json::json!("00".repeat(32));
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&bob_token),
                    Some(wrong_digest)
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        // The publisher's own receipt is recorded but never independent.
        let (status, own, _) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&alice_token),
                Some(receipt(&skill, 3, 5, "clean", "1")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{own}");
        assert_eq!(own["receipt"]["independent"], false);
        assert_eq!(
            own["receipt"]["evaluator"]["id"],
            serde_json::json!(alice.id)
        );
        assert!(own["receipt"].get("result").is_none());
        // Concurrent independent receipts: exactly one verification.
        let (first, second) = tokio::join!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&bob_token),
                    Some(receipt(&skill, 3, 4, "clean", "1"))
                )
            ),
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&carol_token),
                    Some(receipt(&skill, 3, 4, "clean", "1"))
                )
            )
        );
        assert_eq!(first.0, StatusCode::CREATED, "{}", first.2);
        assert_eq!(second.0, StatusCode::CREATED, "{}", second.2);
        let transitions = [
            first.1["transition"].as_str().unwrap(),
            second.1["transition"].as_str().unwrap(),
        ];
        assert!(
            transitions.contains(&"verified") && transitions.contains(&"none"),
            "{transitions:?}"
        );
        assert_eq!(store.events(Some("skill.verified")).await.unwrap().len(), 1);
        let (_, current, _) = send(
            &router,
            request("GET", &format!("/v1/skills/{id}"), Some(&bob_token), None),
        )
        .await;
        assert_eq!(current["status"], "verified");
        assert_eq!(current["summary"]["independent_evaluators"], 2);
        assert_eq!(current["summary"]["input_units_delta"], -200);
        // Duplicates conflict; the same principal counts once even with a new protocol.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&bob_token),
                    Some(receipt(&skill, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&bob_token),
                    Some(receipt(&skill, 3, 4, "clean", "2"))
                )
            )
            .await
            .0,
            StatusCode::CREATED
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{id}"), Some(&bob_token), None)
            )
            .await
            .1["summary"]["independent_evaluators"],
            2
        );
        // A safety failure deprecates finally; a late positive receipt changes nothing.
        let (_, dave_token) = principal(&store, "Dave", "team").await;
        let (status, failed, _) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&dave_token),
                Some(receipt(&skill, 3, 4, "leaked", "1")),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(failed["transition"], "deprecated:safety_evaluation_failed");
        assert_eq!(failed["skill"]["status"], "deprecated");
        let (_, erin_token) = principal(&store, "Erin", "team").await;
        let (_, late, _) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&erin_token),
                Some(receipt(&skill, 3, 5, "clean", "1")),
            ),
        )
        .await;
        assert_eq!(late["transition"], "none");
        assert_eq!(late["skill"]["status"], "deprecated");
        assert_eq!(
            store.events(Some("skill.deprecated")).await.unwrap().len(),
            1
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/deprecate"),
                    Some(&alice_token),
                    Some(serde_json::json!({"reason": "again"}))
                )
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        // A skill deprecated by hand is final too.
        let (_, other, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(
                    "Keep diagnostics on standard error so pipelines stay parseable.",
                    SkillVisibility::Team,
                )),
            ),
        )
        .await;
        let other_id = other["id"].as_str().unwrap();
        let (status, deprecated, _) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{other_id}/deprecate"),
                Some(&bob_token),
                Some(serde_json::json!({"reason": "superseded"})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{deprecated}");
        assert_eq!(deprecated["status"], "deprecated");
        let artifact: SkillArtifact = serde_json::from_value(deprecated).unwrap();
        assert_eq!(artifact.status, SkillStatus::Deprecated);
        let _: SkillPublication =
            serde_json::from_value(publication(LESSON, SkillVisibility::Team)).unwrap();
        let _: SkillReceiptSubmission =
            serde_json::from_value(receipt(&skill, 3, 4, "clean", "1")).unwrap();
        let _ = SkillProvenance::default();
    }

    #[tokio::test]
    async fn public_skill_status_changes_only_through_authorized_evaluators() {
        let (store, router) = registry().await;
        let (alice, alice_token) = principal(&store, "Alice", "team").await;
        let (_, y1) = principal(&store, "Y1", "community-team").await;
        let (_, y2) = principal(&store, "Y2", "community-team").await;
        let authorized = |name: &'static str| {
            let store = store.clone();
            async move {
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap()
            }
        };
        let (_e1, e1_token) = authorized("E1").await;
        let (_e2, e2_token) = authorized("E2").await;
        let (e3, e3_token) = authorized("E3").await;
        // Capabilities are server-side: /v1/me reports them and no body may claim them.
        assert_eq!(
            send(&router, request("GET", "/v1/me", Some(&e1_token), None))
                .await
                .1["authorized_evaluator"],
            true
        );
        assert_eq!(
            send(&router, request("GET", "/v1/me", Some(&y1), None))
                .await
                .1["authorized_evaluator"],
            false
        );
        let mut forged_publication = publication(LESSON, SkillVisibility::Public);
        forged_publication["authorized_evaluator"] = serde_json::json!(true);
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills",
                    Some(&alice_token),
                    Some(forged_publication)
                )
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let (status, skill) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{skill}");
        let id = skill["id"].as_str().unwrap().to_owned();
        // An ordinary outsider never sees a public candidate; an authorized evaluator does.
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{id}"), Some(&y1), None)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&y1),
                    Some(receipt(&skill, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{id}"), None, None)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{id}"), Some(&e1_token), None)
            )
            .await
            .0,
            StatusCode::OK
        );
        for field in [
            "authoritative",
            "authorized_evaluator",
            "role",
            "trusted",
            "evaluator_principal_id",
        ] {
            let mut forged = receipt(&skill, 3, 4, "clean", "1");
            forged[field] = serde_json::json!(true);
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{id}/evaluations"),
                        Some(&e1_token),
                        Some(forged)
                    )
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
        }
        // Two authorized independent evaluators verify the public candidate.
        let (status, first) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&e1_token),
                Some(receipt(&skill, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{first}");
        assert_eq!(first["receipt"]["authoritative"], true);
        assert_eq!(first["receipt"]["independent"], true);
        assert_eq!(first["transition"], "none");
        let (_, second) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&e2_token),
                Some(receipt(&skill, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(second["transition"], "verified", "{second}");
        // Ordinary cross-team principals may file community receipts on the public verified skill,
        // which are stored and shown but never move the status.
        let (status, community) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&y1),
                Some(receipt(&skill, 3, 5, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{community}");
        assert_eq!(community["receipt"]["authoritative"], false);
        assert_eq!(community["transition"], "none");
        let (status, leaked) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&y2),
                Some(receipt(&skill, 3, 4, "leaked", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{leaked}");
        assert_eq!(
            leaked["transition"], "none",
            "an outsider's safety failure never deprecates"
        );
        assert_eq!(leaked["skill"]["status"], "verified");
        let (_, current) = send(
            &router,
            request("GET", &format!("/v1/skills/{id}"), None, None),
        )
        .await
        .into_status_and_body();
        assert_eq!(current["status"], "verified");
        assert_eq!(current["summary"]["independent_evaluators"], 2);
        assert_eq!(current["summary"]["safety_failures"], 0);
        assert_eq!(current["summary"]["community_receipts"], 2);
        assert_eq!(current["summary"]["community_safety_failures"], 1);
        assert!(
            store
                .events(Some("skill.deprecated"))
                .await
                .unwrap()
                .is_empty()
        );
        // The same authorized principal counts once, whatever its protocol count.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(&e1_token),
                    Some(receipt(&skill, 3, 4, "clean", "2"))
                )
            )
            .await
            .0,
            StatusCode::CREATED
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{id}"), None, None)
            )
            .await
            .1["summary"]["independent_evaluators"],
            2
        );
        // An authorized evaluator's safety failure deprecates, finally.
        let (_, failed) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/evaluations"),
                Some(&e2_token),
                Some(receipt(&skill, 3, 4, "leaked", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(failed["transition"], "deprecated:safety_evaluation_failed");
        assert_eq!(
            store.events(Some("skill.deprecated")).await.unwrap().len(),
            1
        );
        // A disabled authorized evaluator stops counting: its earlier receipt no longer helps the gate.
        let second_lesson = "Keep diagnostics on standard error so pipelines stay parseable.";
        let (_, other) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(second_lesson, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        let other_id = other["id"].as_str().unwrap().to_owned();
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{other_id}/evaluations"),
                    Some(&e3_token),
                    Some(receipt(&other, 3, 4, "clean", "1"))
                )
            )
            .await
            .1["receipt"]["authoritative"],
            true
        );
        store.disable_principal(&e3.id).await.unwrap();
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{other_id}/evaluations"),
                    Some(&e3_token),
                    Some(receipt(&other, 3, 4, "clean", "2"))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (_, after_disable) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{other_id}/evaluations"),
                Some(&e1_token),
                Some(receipt(&other, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            after_disable["transition"], "none",
            "a disabled evaluator's receipt no longer counts: {after_disable}"
        );
        assert_eq!(
            after_disable["skill"]["summary"]["independent_evaluators"],
            1
        );
        let listed = send(
            &router,
            request(
                "GET",
                &format!("/v1/skills/{other_id}/evaluations"),
                Some(&e1_token),
                None,
            ),
        )
        .await
        .1;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["evaluator"]["id"] == serde_json::json!(e3.id)
                    && r["authoritative"] == false)
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{other_id}/evaluations"),
                    Some(&e2_token),
                    Some(receipt(&other, 3, 4, "clean", "1"))
                )
            )
            .await
            .1["transition"],
            "verified"
        );
        // Authority is live: revoking the capability stops an earlier receipt from counting,
        // granting it again makes the receipt count, and the gate reflects that on its next run.
        let fourth_lesson = "Report malformed input on standard error and exit non-zero.";
        let (_, fourth) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(fourth_lesson, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        let fourth_id = fourth["id"].as_str().unwrap().to_owned();
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{fourth_id}/evaluations"),
                    Some(&e1_token),
                    Some(receipt(&fourth, 3, 4, "clean", "1"))
                )
            )
            .await
            .1["receipt"]["authoritative"],
            true
        );
        let e1_id = send(&router, request("GET", "/v1/me", Some(&e1_token), None))
            .await
            .1["id"]
            .as_str()
            .unwrap()
            .to_owned();
        store.set_authorized_evaluator(&e1_id, false).await.unwrap();
        let listed = send(
            &router,
            request(
                "GET",
                &format!("/v1/skills/{fourth_id}/evaluations"),
                Some(&alice_token),
                None,
            ),
        )
        .await
        .1;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["authoritative"] == false),
            "a revoked evaluator's receipt no longer counts: {listed}"
        );
        let (_, after_revoke) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{fourth_id}/evaluations"),
                Some(&e2_token),
                Some(receipt(&fourth, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(after_revoke["transition"], "none", "{after_revoke}");
        assert_eq!(
            after_revoke["skill"]["summary"]["independent_evaluators"],
            1
        );
        assert_eq!(after_revoke["skill"]["summary"]["community_receipts"], 1);
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{fourth_id}"),
                    Some(&e1_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "a revoked evaluator no longer sees public candidates"
        );
        store.set_authorized_evaluator(&e1_id, true).await.unwrap();
        let (_, teammate_token) = principal(&store, "Alice's teammate", "team").await;
        let (_, after_regrant) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{fourth_id}/evaluations"),
                Some(&teammate_token),
                Some(receipt(&fourth, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            after_regrant["transition"], "verified",
            "authority restored counts again: {after_regrant}"
        );
        assert_eq!(
            after_regrant["skill"]["summary"]["independent_evaluators"],
            3
        );
        // A later grant never promotes a receipt filed as a community receipt: Y1's earlier
        // community receipts on the first skill stay community after Y1 is authorized.
        let y1_id = send(&router, request("GET", "/v1/me", Some(&y1), None))
            .await
            .1["id"]
            .as_str()
            .unwrap()
            .to_owned();
        store.set_authorized_evaluator(&y1_id, true).await.unwrap();
        let (_, first_after_grant) = send(
            &router,
            request("GET", &format!("/v1/skills/{id}"), None, None),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            first_after_grant["summary"]["community_receipts"], 2,
            "{first_after_grant}"
        );
        assert_eq!(first_after_grant["summary"]["independent_evaluators"], 2);
        store.set_authorized_evaluator(&y1_id, false).await.unwrap();
        // The publisher never counts as independent, even with the capability.
        store
            .set_authorized_evaluator(&alice.id, true)
            .await
            .unwrap();
        let third_lesson =
            "Return a distinct exit status for malformed input and say so on standard error.";
        let (_, third) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(third_lesson, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        let third_id = third["id"].as_str().unwrap().to_owned();
        let (_, own) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{third_id}/evaluations"),
                Some(&alice_token),
                Some(receipt(&third, 3, 5, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(own["receipt"]["independent"], false);
        assert_eq!(own["receipt"]["authoritative"], true);
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{third_id}/evaluations"),
                    Some(&e1_token),
                    Some(receipt(&third, 3, 4, "clean", "1"))
                )
            )
            .await
            .1["transition"],
            "none"
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{third_id}/evaluations"),
                    Some(&e2_token),
                    Some(receipt(&third, 3, 4, "clean", "1"))
                )
            )
            .await
            .1["transition"],
            "verified"
        );
        // Team skills: only the team evaluates; an authorized evaluator of another team cannot even see them.
        let (_, team_skill) = send(&router, request("POST", "/v1/skills", Some(&alice_token), Some(publication("Prefer explicit exit codes over printed error prose when scripts are composed.", SkillVisibility::Team)))).await.into_status_and_body();
        let team_id = team_skill["id"].as_str().unwrap();
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}"),
                    Some(&e1_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{team_id}/evaluations"),
                    Some(&e1_token),
                    Some(receipt(&team_skill, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }

    fn challenge(kind: &str, claim: &str, evidence: Option<&str>) -> serde_json::Value {
        serde_json::json!({"kind": kind, "claim": claim, "applicability": "Library callers embedding the tool.", "evidence_receipt_id": evidence})
    }

    #[tokio::test]
    async fn challenges_are_scoped_immutable_evidence_bound_and_never_move_status() {
        let (store, router) = registry().await;
        let (_, alice_token) = principal(&store, "Alice", "team").await;
        let (bob, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let (_, x_token) = principal(&store, "Xavier", "other-team").await;
        let (_, team_skill) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Team)),
            ),
        )
        .await
        .into_status_and_body();
        let team_id = team_skill["id"].as_str().unwrap().to_owned();
        let public_lesson =
            "Prefer explicit exit codes over printed error prose when scripts are composed.";
        let (_, public_skill) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(public_lesson, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        let public_id = public_skill["id"].as_str().unwrap().to_owned();
        let (_, bob_receipt) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/evaluations"),
                Some(&bob_token),
                Some(receipt(&public_skill, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/evaluations"),
                Some(&carol_token),
                Some(receipt(&public_skill, 3, 4, "clean", "1")),
            ),
        )
        .await;
        let bob_receipt_id = bob_receipt["receipt"]["id"].as_str().unwrap().to_owned();
        let claim = "The exit-status rule fails when the tool is used as a library: callers never see the process status.";
        // Writes need a token; a challenge on a skill the principal cannot see is not found.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    None,
                    Some(challenge("applicability_failure", claim, None))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{team_id}/challenges"),
                    Some(&x_token),
                    Some(challenge("applicability_failure", claim, None))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills/skill_missing/challenges",
                    Some(&bob_token),
                    Some(challenge("applicability_failure", claim, None))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        // The challenger is the token's principal; a body naming one, or a status, is refused; so is a free-form kind.
        for (field, value) in [
            ("challenger", serde_json::json!({"id": "mallory"})),
            ("status", serde_json::json!("addressed")),
            ("evidence_backed", serde_json::json!(true)),
            ("verdict", serde_json::json!("wrong")),
        ] {
            let mut forged = challenge("applicability_failure", claim, None);
            forged[field] = value;
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{public_id}/challenges"),
                        Some(&x_token),
                        Some(forged)
                    )
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
        }
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    Some(&x_token),
                    Some(challenge("rant", claim, None))
                )
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    Some(&x_token),
                    Some(challenge(
                        "applicability_failure",
                        "Check /home/alice/config.toml first.",
                        None
                    ))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "challenge text is sanitized like a lesson"
        );
        // A text-only challenge by an outsider on a public verified skill is recorded and changes nothing.
        let (status, opened) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/challenges"),
                Some(&x_token),
                Some(challenge("applicability_failure", claim, None)),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        assert_eq!(opened["challenger"]["display_name"], "Xavier");
        assert_eq!(opened["kind"], "applicability_failure");
        assert_eq!(opened["status"], "open");
        assert_eq!(opened["evidence_backed"], false);
        assert_eq!(opened["skill_version"], 1);
        assert_eq!(opened["content_digest"], public_skill["content_digest"]);
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{public_id}"), None, None)
            )
            .await
            .1["status"],
            "verified"
        );
        // The same claim by the same principal is one challenge; another principal's is a second.
        let (status, again) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/challenges"),
                Some(&x_token),
                Some(challenge("applicability_failure", claim, None)),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["id"], opened["id"]);
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    Some(&bob_token),
                    Some(challenge("applicability_failure", claim, None))
                )
            )
            .await
            .0,
            StatusCode::CREATED
        );
        // Evidence must be a receipt recorded on this very skill.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    Some(&bob_token),
                    Some(challenge(
                        "negative_transfer",
                        "Fails on the atomic-state family.",
                        Some("receipt_missing")
                    ))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (_, team_receipt) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{team_id}/evaluations"),
                Some(&bob_token),
                Some(receipt(&team_skill, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        let team_receipt_id = team_receipt["receipt"]["id"].as_str().unwrap();
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{public_id}/challenges"),
                    Some(&bob_token),
                    Some(challenge(
                        "negative_transfer",
                        "Fails on the atomic-state family.",
                        Some(team_receipt_id)
                    ))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "a receipt of another skill is not evidence here"
        );
        let (status, backed) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/challenges"),
                Some(&bob_token),
                Some(challenge(
                    "negative_transfer",
                    "Fails on the atomic-state family.",
                    Some(&bob_receipt_id),
                )),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{backed}");
        assert_eq!(backed["evidence_backed"], true);
        assert_eq!(
            backed["evidence"]["receipt_id"],
            serde_json::json!(bob_receipt_id)
        );
        assert_eq!(
            backed["evidence"]["evaluator"]["id"],
            serde_json::json!(bob.id)
        );
        assert_eq!(backed["evidence"]["safety"], "clean");
        assert_eq!(backed["evidence"]["authoritative"], true);
        // Visibility follows the skill: the public skill's challenges are readable by anyone,
        // the team skill's only by the team.
        let listed = send(
            &router,
            request(
                "GET",
                &format!("/v1/skills/{public_id}/challenges"),
                None,
                None,
            ),
        )
        .await
        .1;
        assert_eq!(listed.as_array().unwrap().len(), 3);
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["evidence_backed"] == true)
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{team_id}/challenges"),
                    Some(&carol_token),
                    Some(challenge(
                        "generalization_failure",
                        "Only holds for Python tools.",
                        None
                    ))
                )
            )
            .await
            .0,
            StatusCode::CREATED
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}/challenges"),
                    Some(&x_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}/challenges"),
                    None,
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}/challenges"),
                    Some(&alice_token),
                    None
                )
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{team_id}"),
                    Some(&alice_token),
                    None
                )
            )
            .await
            .1["status"],
            "candidate",
            "a text challenge never moves a candidate either"
        );
        // An evidence-backed safety challenge flows through the safety gate: it is the referenced
        // receipt that deprecated the skill, and the challenge merely points at it.
        let (_, failed) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/evaluations"),
                Some(&carol_token),
                Some(receipt(&public_skill, 3, 4, "leaked", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(failed["transition"], "deprecated:safety_evaluation_failed");
        let failed_receipt_id = failed["receipt"]["id"].as_str().unwrap();
        let (status, safety) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{public_id}/challenges"),
                Some(&carol_token),
                Some(challenge(
                    "safety_concern",
                    "The candidate arm leaked the harmful rule into requests.",
                    Some(failed_receipt_id),
                )),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            status,
            StatusCode::CREATED,
            "a deprecated skill may still be challenged, against its exact version"
        );
        assert_eq!(safety["evidence"]["safety"], "failed");
        assert_eq!(safety["evidence_backed"], true);
        assert_eq!(
            store.events(Some("skill.deprecated")).await.unwrap().len(),
            1,
            "the challenge itself deprecated nothing"
        );
        let events =
            serde_json::to_string(&store.events(Some("challenge.created")).await.unwrap()).unwrap();
        assert!(
            !events.contains(claim) && !events.contains(&x_token),
            "events carry ids, never claim text or tokens"
        );
        let _: ChallengeItem = serde_json::from_value(safety).unwrap();
    }

    fn fork_body(lesson: &str, applicability: &str, challenge: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "lesson": lesson,
            "applicability": applicability,
            "content_digest": skill_content_digest(lesson, applicability, SKILL_SANITIZATION_VERSION),
            "sanitization_version": SKILL_SANITIZATION_VERSION,
            "provenance": {"source_kind": "refinement", "task_family": "cli-error-contract"},
            "responding_to_challenge_id": challenge,
        })
    }

    #[tokio::test]
    async fn forks_are_immutable_server_derived_and_form_a_lineage() {
        let (store, router) = registry().await;
        let (alice, alice_token) = principal(&store, "Alice", "team").await;
        let (_, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let (erin, erin_token) = principal(&store, "Erin", "other-team").await;
        let (_, public_skill) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(LESSON, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        let s1 = public_skill["id"].as_str().unwrap().to_owned();
        for token in [&bob_token, &carol_token] {
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1}/evaluations"),
                    Some(token),
                    Some(receipt(&public_skill, 3, 4, "clean", "1")),
                ),
            )
            .await;
        }
        let (_, team_skill) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(
                    "Keep diagnostics on standard error so pipelines stay parseable.",
                    SkillVisibility::Team,
                )),
            ),
        )
        .await
        .into_status_and_body();
        let t1 = team_skill["id"].as_str().unwrap().to_owned();
        let (_, opened) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1}/challenges"),
                Some(&erin_token),
                Some(challenge(
                    "applicability_failure",
                    "Fails for tools used as libraries.",
                    None,
                )),
            ),
        )
        .await
        .into_status_and_body();
        let challenge_id = opened["id"].as_str().unwrap().to_owned();
        let narrowed = "Tools whose callers rely on exit status and stderr diagnostics and run them as processes.";
        // Lineage is server-derived: bodies naming parent, version, status, visibility or forker are refused.
        for (field, value) in [
            ("parent_skill_id", serde_json::json!("skill_other")),
            ("version", serde_json::json!(9)),
            ("status", serde_json::json!("verified")),
            ("visibility", serde_json::json!("team")),
            ("forked_by", serde_json::json!({"id": alice.id})),
            ("superseded_by", serde_json::json!("x")),
        ] {
            let mut forged = fork_body(LESSON, narrowed, None);
            forged[field] = value;
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{s1}/forks"),
                        Some(&carol_token),
                        Some(forged)
                    )
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
        }
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1}/forks"),
                    None,
                    Some(fork_body(LESSON, narrowed, None))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{t1}/forks"),
                    Some(&erin_token),
                    Some(fork_body(LESSON, narrowed, None))
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND,
            "an invisible parent cannot be forked"
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&carol_token),
                    Some(fork_body(LESSON, APPLICABILITY, None))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "a fork must change something"
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&carol_token),
                    Some(fork_body(LESSON, narrowed, Some("challenge_missing")))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let (_, other_challenge) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{t1}/challenges"),
                Some(&bob_token),
                Some(challenge(
                    "correctness_failure",
                    "Wrong for pipelines.",
                    None,
                )),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&carol_token),
                    Some(fork_body(LESSON, narrowed, other_challenge["id"].as_str()))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "the challenge must be on the parent"
        );
        // A team member's fork answering Erin's challenge: version 2, parent S1, inherited visibility.
        let (status, s2a) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1}/forks"),
                Some(&carol_token),
                Some(fork_body(LESSON, narrowed, Some(&challenge_id))),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{s2a}");
        assert_eq!(s2a["status"], "candidate");
        assert_eq!(s2a["version"], 2);
        assert_eq!(s2a["parent_skill_id"], serde_json::json!(s1));
        assert_eq!(s2a["visibility"], "public");
        assert_eq!(s2a["shared_scope"]["team_id"], "team");
        assert_eq!(s2a["forked_by"]["display_name"], "Carol");
        assert_eq!(
            s2a["responding_to_challenge_id"],
            serde_json::json!(challenge_id)
        );
        assert!(s2a.get("superseded_by").is_none());
        let s2a_id = s2a["id"].as_str().unwrap().to_owned();
        // A retry is the same fork; another team's fork of the public skill lives in its own team and is public.
        let (status, again) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1}/forks"),
                Some(&carol_token),
                Some(fork_body(LESSON, narrowed, Some(&challenge_id))),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["id"], s2a["id"]);
        let (status, s2b) = send(&router, request("POST", &format!("/v1/skills/{s1}/forks"), Some(&erin_token), Some(fork_body("Prefer explicit exit codes over printed error prose when scripts are composed.", APPLICABILITY, None)))).await.into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{s2b}");
        assert_eq!(s2b["visibility"], "public");
        assert_eq!(s2b["shared_scope"]["team_id"], "other-team");
        assert_eq!(s2b["publisher"]["id"], serde_json::json!(erin.id));
        let s2b_id = s2b["id"].as_str().unwrap().to_owned();
        // Identical content that already exists as a different skill in the team is a conflict, not a fork.
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{t1}/forks"),
                    Some(&bob_token),
                    Some(fork_body(LESSON, narrowed, None))
                )
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        // The parent is untouched and S1 stays the active skill while both forks are candidates.
        let (_, parent) = send(
            &router,
            request("GET", &format!("/v1/skills/{s1}"), None, None),
        )
        .await
        .into_status_and_body();
        assert_eq!(parent["status"], "verified");
        assert_eq!(parent["version"], 1);
        assert!(parent.get("superseded_by").is_none());
        // Candidate forks follow candidate visibility: their own team and authorized evaluators
        // see them, anonymous readers do not; the lineage view is filtered the same way.
        let ids = |value: serde_json::Value| {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f["id"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(send(
                &router,
                request("GET", &format!("/v1/skills/{s1}/forks"), None, None)
            )
            .await
            .1),
            Vec::<serde_json::Value>::new()
        );
        assert_eq!(
            ids(send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&alice_token),
                    None
                )
            )
            .await
            .1),
            vec![serde_json::json!(s2a_id)]
        );
        assert_eq!(
            ids(send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&erin_token),
                    None
                )
            )
            .await
            .1),
            vec![serde_json::json!(s2b_id)]
        );
        let (_, evaluator_token) = store
            .create_principal_with("Eval", "org", "eval-team", true)
            .await
            .unwrap();
        assert_eq!(
            ids(send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{s1}/forks"),
                    Some(&evaluator_token),
                    None
                )
            )
            .await
            .1),
            vec![serde_json::json!(s2a_id), serde_json::json!(s2b_id)]
        );
        let (status, lineage) = send(
            &router,
            request(
                "GET",
                &format!("/v1/skills/{s2b_id}/lineage"),
                Some(&evaluator_token),
                None,
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::OK, "{lineage}");
        assert_eq!(lineage["root_id"], serde_json::json!(s1));
        assert_eq!(lineage["requested_id"], serde_json::json!(s2b_id));
        assert_eq!(
            lineage["active_id"],
            serde_json::Value::Null,
            "a candidate fork is not active"
        );
        let anonymous_lineage = send(
            &router,
            request("GET", &format!("/v1/skills/{s1}/lineage"), None, None),
        )
        .await
        .1;
        assert_eq!(anonymous_lineage["active_id"], serde_json::json!(s1));
        assert_eq!(
            anonymous_lineage["nodes"].as_array().unwrap().len(),
            1,
            "anonymous readers see no candidate forks"
        );
        let nodes = lineage["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0]["id"], serde_json::json!(s1));
        assert_eq!(nodes[0]["open_challenges"], 1);
        assert!(nodes.iter().any(|n| n["id"] == serde_json::json!(s2a_id)
            && n["responding_to_challenge_id"] == serde_json::json!(challenge_id)
            && n["version"] == 2));
        // A team-visibility fork of a team skill stays inside the team, in listings and lineage alike.
        let (status, team_fork) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{t1}/forks"),
                Some(&bob_token),
                Some(fork_body(
                    "Keep diagnostics on standard error so pipelines stay parseable and scripted.",
                    APPLICABILITY,
                    None,
                )),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{team_fork}");
        assert_eq!(team_fork["visibility"], "team");
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{t1}/forks"),
                    Some(&erin_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{}/lineage", team_fork["id"].as_str().unwrap()),
                    Some(&erin_token),
                    None
                )
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &router,
                request(
                    "GET",
                    &format!("/v1/skills/{t1}/lineage"),
                    Some(&alice_token),
                    None
                )
            )
            .await
            .1["nodes"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // Ordinary publication never declares lineage.
        let mut lineage_claim = publication(
            "Return a distinct exit status for malformed input and say so on standard error.",
            SkillVisibility::Team,
        );
        lineage_claim["parent_skill_id"] = serde_json::json!(s1);
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    "/v1/skills",
                    Some(&alice_token),
                    Some(lineage_claim)
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(store.events(Some("skill.forked")).await.unwrap().len(), 3);
        let _: Lineage = serde_json::from_value(lineage).unwrap();
        let _: SkillArtifact = serde_json::from_value(s2a).unwrap();
    }

    fn comparison(
        fork: &serde_json::Value,
        parent: &serde_json::Value,
        parent_passes: u32,
        fork_passes: u32,
        fork_safety: &str,
        version: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "fork_id": fork["id"],
            "parent_id": parent["id"],
            "fork_content_digest": fork["content_digest"],
            "parent_content_digest": parent["content_digest"],
            "protocol_version": 1,
            "task_family": "cli-error-contract",
            "held_out_tasks": [{"track": "project", "id": "service-config-checker", "protected_sha256": "cd".repeat(32)}],
            "catalog_revision": "catalog-1",
            "s_code_revision": "rev-1",
            "provider": "openai-compatible",
            "model": "model-x",
            "repeats": 5,
            "parent": [outcome("service-config-checker", 5, parent_passes, 1000)],
            "fork": [outcome("service-config-checker", 5, fork_passes, 900)],
            "parent_safety": {"verdict": "clean", "candidate_retrieved_only_skill": true, "harmful_rule_absent_from_requests": true},
            "fork_safety": {"verdict": fork_safety, "candidate_retrieved_only_skill": fork_safety == "clean", "harmful_rule_absent_from_requests": fork_safety == "clean"},
            "artifact_references": ["runs/compare"],
            "evaluator": {"name": "s-code-skill-comparator", "version": version},
        })
    }

    /// Publish a public skill as `publisher`, verify it with two team receipts, return it.
    async fn verified_public(
        router: &Router,
        publisher: &str,
        verifiers: [&str; 2],
        lesson: &str,
    ) -> serde_json::Value {
        let (status, skill) = send(
            router,
            request(
                "POST",
                "/v1/skills",
                Some(publisher),
                Some(publication(lesson, SkillVisibility::Public)),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{skill}");
        let id = skill["id"].as_str().unwrap();
        for token in verifiers {
            let (status, _) = send(
                router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(token),
                    Some(receipt(&skill, 3, 4, "clean", "1")),
                ),
            )
            .await
            .into_status_and_body();
            assert_eq!(status, StatusCode::CREATED);
        }
        send(
            router,
            request("GET", &format!("/v1/skills/{id}"), Some(publisher), None),
        )
        .await
        .1
    }

    async fn verify_fork(router: &Router, fork: &serde_json::Value, verifiers: [&str; 2]) {
        let id = fork["id"].as_str().unwrap();
        for token in verifiers {
            let (status, body) = send(
                router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(token),
                    Some(receipt(fork, 3, 4, "clean", "1")),
                ),
            )
            .await
            .into_status_and_body();
            assert_eq!(status, StatusCode::CREATED, "{body}");
        }
    }

    #[tokio::test]
    async fn forks_supersede_parents_only_through_the_comparative_gate() {
        let (store, router) = registry().await;
        let (_, alice_token) = principal(&store, "Alice", "team").await;
        let (_, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let (_, dave_token) = principal(&store, "Dave", "team-d").await;
        let (_, erin_token) = principal(&store, "Erin", "team-e").await;
        let (_, hana_token) = principal(&store, "Hana", "team-h").await;
        let (_, f_token) = store
            .create_principal_with("Fay", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, g_token) = store
            .create_principal_with("Gus", "org", "eval-team", true)
            .await
            .unwrap();
        // Scenario 1: S1 verified; D finds an applicability failure and challenges it; E forks S2 with a
        // narrowed applicability answering the challenge; F and G verify S2 and compare S1 against S2.
        let s1 = verified_public(&router, &alice_token, [&bob_token, &carol_token], LESSON).await;
        let s1_id = s1["id"].as_str().unwrap().to_owned();
        let (_, challenge_d) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1_id}/challenges"),
                Some(&dave_token),
                Some(challenge(
                    "applicability_failure",
                    "Fails for tools used as libraries: callers never see the process status.",
                    None,
                )),
            ),
        )
        .await
        .into_status_and_body();
        let challenge_id = challenge_d["id"].as_str().unwrap().to_owned();
        let narrowed = "Tools whose callers rely on exit status and stderr diagnostics and run them as processes.";
        let (status, s2) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1_id}/forks"),
                Some(&erin_token),
                Some(fork_body(LESSON, narrowed, Some(&challenge_id))),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{s2}");
        let s2_id = s2["id"].as_str().unwrap().to_owned();
        // Comparisons never carry a verdict; a comparison on a non-fork or with a wrong digest is refused.
        for (field, value) in [
            ("winner", serde_json::json!("fork")),
            ("better", serde_json::json!(true)),
            ("supersede", serde_json::json!(true)),
            ("score", serde_json::json!(0.9)),
            ("superseded_by", serde_json::json!(s2_id)),
            ("independent", serde_json::json!(true)),
        ] {
            let mut forged = comparison(&s2, &s1, 3, 4, "clean", "1");
            forged[field] = value;
            assert_eq!(
                send(
                    &router,
                    request(
                        "POST",
                        &format!("/v1/skills/{s2_id}/comparisons"),
                        Some(&f_token),
                        Some(forged)
                    )
                )
                .await
                .0,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{field}"
            );
        }
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s1_id}/comparisons"),
                    Some(&f_token),
                    Some(comparison(&s2, &s1, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST,
            "S1 is not a fork"
        );
        let mut wrong = comparison(&s2, &s1, 3, 4, "clean", "1");
        wrong["parent_content_digest"] = serde_json::json!("00".repeat(32));
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s2_id}/comparisons"),
                    Some(&f_token),
                    Some(wrong)
                )
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s2_id}/comparisons"),
                    None,
                    Some(comparison(&s2, &s1, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        // A comparison recorded while the fork is still a candidate is kept but supersedes nothing.
        let (status, early) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s2_id}/comparisons"),
                Some(&f_token),
                Some(comparison(&s2, &s1, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{early}");
        assert_eq!(early["transition"], "none");
        assert_eq!(early["comparison"]["authoritative"], true);
        assert_eq!(early["comparison"]["independent"], true);
        assert_eq!(early["comparison"]["verdict"]["fork_passes"], 4);
        assert!(early["parent"].get("superseded_by").is_none());
        assert_eq!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s2_id}/comparisons"),
                    Some(&f_token),
                    Some(comparison(&s2, &s1, 3, 4, "clean", "1"))
                )
            )
            .await
            .0,
            StatusCode::CONFLICT,
            "comparisons are immutable"
        );
        // The forker's own comparison is never independent; an ordinary outsider's is a community comparison.
        verify_fork(&router, &s2, [&f_token, &g_token]).await;
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{s2_id}"), None, None)
            )
            .await
            .1["status"],
            "verified"
        );
        let (_, own) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s2_id}/comparisons"),
                Some(&erin_token),
                Some(comparison(&s2, &s1, 3, 5, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(own["comparison"]["independent"], false);
        assert_eq!(own["transition"], "none");
        let (_, community) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s2_id}/comparisons"),
                Some(&dave_token),
                Some(comparison(&s2, &s1, 3, 5, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(community["comparison"]["authoritative"], false);
        assert_eq!(community["transition"], "none");
        assert!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{s1_id}"), None, None)
            )
            .await
            .1
            .get("superseded_by")
            .is_none()
        );
        // G's independent authoritative comparison completes the gate: S2 supersedes S1, S1 stays
        // verified and inspectable, the challenge is addressed, and the active successor is S2.
        let (status, decided) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s2_id}/comparisons"),
                Some(&g_token),
                Some(comparison(&s2, &s1, 3, 4, "clean", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{decided}");
        assert_eq!(decided["transition"], "superseded");
        assert_eq!(decided["parent"]["superseded_by"], serde_json::json!(s2_id));
        assert_eq!(decided["parent"]["status"], "verified");
        let (_, s1_now) = send(
            &router,
            request("GET", &format!("/v1/skills/{s1_id}"), None, None),
        )
        .await
        .into_status_and_body();
        assert_eq!(s1_now["superseded_by"], serde_json::json!(s2_id));
        assert_eq!(s1_now["status"], "verified", "superseded is not deprecated");
        let challenges = send(
            &router,
            request("GET", &format!("/v1/skills/{s1_id}/challenges"), None, None),
        )
        .await
        .1;
        assert_eq!(challenges[0]["status"], "addressed");
        assert_eq!(
            challenges[0]["addressed_by_skill_id"],
            serde_json::json!(s2_id)
        );
        let lineage = send(
            &router,
            request("GET", &format!("/v1/skills/{s1_id}/lineage"), None, None),
        )
        .await
        .1;
        assert_eq!(lineage["active_id"], serde_json::json!(s2_id));
        assert_eq!(
            store.events(Some("skill.superseded")).await.unwrap().len(),
            1
        );
        // Scenario 2: a competing fork S2b verified and compared later passes its own gate but the
        // parent already has a successor: the first committed supersession wins, deterministically.
        let (_, s2b) = send(&router, request("POST", &format!("/v1/skills/{s1_id}/forks"), Some(&hana_token), Some(fork_body("Prefer explicit exit codes over printed error prose when scripts are composed.", APPLICABILITY, None)))).await.into_status_and_body();
        let s2b_id = s2b["id"].as_str().unwrap().to_owned();
        verify_fork(&router, &s2b, [&f_token, &g_token]).await;
        for token in [&f_token, &g_token] {
            let (_, body) = send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{s2b_id}/comparisons"),
                    Some(token),
                    Some(comparison(&s2b, &s1, 3, 5, "clean", "1")),
                ),
            )
            .await
            .into_status_and_body();
            assert_eq!(body["transition"], "none", "{body}");
        }
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{s1_id}"), None, None)
            )
            .await
            .1["superseded_by"],
            serde_json::json!(s2_id)
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{s2b_id}"), None, None)
            )
            .await
            .1["status"],
            "verified",
            "the competing fork stays a verified fork"
        );
        // A late positive receipt on the parent changes nothing; a later safety failure of the parent
        // deprecates it while its independently verified successor remains active (scenario 5).
        let (_, late) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1_id}/evaluations"),
                Some(&carol_token),
                Some(receipt(&s1, 3, 5, "clean", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(late["transition"], "none");
        let (_, failed) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{s1_id}/evaluations"),
                Some(&bob_token),
                Some(receipt(&s1, 3, 4, "leaked", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(failed["transition"], "deprecated:safety_evaluation_failed");
        let lineage = send(
            &router,
            request("GET", &format!("/v1/skills/{s1_id}/lineage"), None, None),
        )
        .await
        .1;
        assert_eq!(
            lineage["active_id"],
            serde_json::json!(s2_id),
            "a deprecated parent does not pull down its verified successor"
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{s2_id}"), None, None)
            )
            .await
            .1["status"],
            "verified"
        );
        let _: ComparisonAccepted = serde_json::from_value(decided).unwrap();
    }

    #[tokio::test]
    async fn bad_or_unsafe_forks_never_supersede_and_safety_wins_races() {
        let (store, router) = registry().await;
        let (_, alice_token) = principal(&store, "Alice", "team").await;
        let (_, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let (_, erin_token) = principal(&store, "Erin", "team-e").await;
        let (_, f_token) = store
            .create_principal_with("Fay", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, g_token) = store
            .create_principal_with("Gus", "org", "eval-team", true)
            .await
            .unwrap();
        // Scenario 3: a worse refinement is verified in its own right yet never supersedes its parent.
        let p = verified_public(&router, &alice_token, [&bob_token, &carol_token], LESSON).await;
        let p_id = p["id"].as_str().unwrap().to_owned();
        let (_, worse) = send(&router, request("POST", &format!("/v1/skills/{p_id}/forks"), Some(&erin_token), Some(fork_body("Prefer explicit exit codes over printed error prose when scripts are composed.", APPLICABILITY, None)))).await.into_status_and_body();
        let worse_id = worse["id"].as_str().unwrap().to_owned();
        verify_fork(&router, &worse, [&f_token, &g_token]).await;
        for token in [&f_token, &g_token] {
            let (_, body) = send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{worse_id}/comparisons"),
                    Some(token),
                    Some(comparison(&worse, &p, 4, 3, "clean", "1")),
                ),
            )
            .await
            .into_status_and_body();
            assert_eq!(body["transition"], "none", "{body}");
        }
        let (_, parent) = send(
            &router,
            request("GET", &format!("/v1/skills/{p_id}"), None, None),
        )
        .await
        .into_status_and_body();
        assert!(parent.get("superseded_by").is_none());
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{p_id}/lineage"), None, None)
            )
            .await
            .1["active_id"],
            serde_json::json!(p_id)
        );
        // Scenario 4: a fork whose own safety evidence fails is deprecated by the ordinary gate and can
        // never supersede, whatever its comparisons say; the parent is unaffected.
        let (_, unsafe_fork) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{p_id}/forks"),
                Some(&erin_token),
                Some(fork_body(
                    "Keep diagnostics on standard error so pipelines stay parseable.",
                    APPLICABILITY,
                    None,
                )),
            ),
        )
        .await
        .into_status_and_body();
        let unsafe_id = unsafe_fork["id"].as_str().unwrap().to_owned();
        verify_fork(&router, &unsafe_fork, [&f_token, &g_token]).await;
        let (_, deprecated) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{unsafe_id}/evaluations"),
                Some(&f_token),
                Some(receipt(&unsafe_fork, 3, 4, "leaked", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(
            deprecated["transition"],
            "deprecated:safety_evaluation_failed"
        );
        for token in [&f_token, &g_token] {
            let (status, body) = send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{unsafe_id}/comparisons"),
                    Some(token),
                    Some(comparison(&unsafe_fork, &p, 3, 5, "clean", "1")),
                ),
            )
            .await
            .into_status_and_body();
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "a deprecated fork is never compared: {body}"
            );
        }
        assert!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{p_id}"), None, None)
            )
            .await
            .1
            .get("superseded_by")
            .is_none()
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{p_id}"), None, None)
            )
            .await
            .1["status"],
            "verified"
        );
        // An authoritative comparison whose fork arm failed safety never
        // counts, and it deprecates the fork like a failed receipt would.
        let (_, leaky) = send(&router, request("POST", &format!("/v1/skills/{p_id}/forks"), Some(&erin_token), Some(fork_body("Return a distinct exit status for malformed input and log the reason on standard error.", APPLICABILITY, None)))).await.into_status_and_body();
        let leaky_id = leaky["id"].as_str().unwrap().to_owned();
        verify_fork(&router, &leaky, [&f_token, &g_token]).await;
        let (status, leaked) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{leaky_id}/comparisons"),
                Some(&f_token),
                Some(comparison(&leaky, &p, 3, 4, "leaked", "1")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{leaked}");
        assert_eq!(leaked["transition"], "none");
        assert_eq!(leaked["fork"]["status"], "deprecated", "{leaked}");
        assert_eq!(
            leaked["fork"]["deprecation_reason"],
            "comparison_safety_failed"
        );
        let (_, good) = send(&router, request("POST", &format!("/v1/skills/{p_id}/forks"), Some(&erin_token), Some(fork_body("Return a distinct exit status for malformed input and say so on standard error.", APPLICABILITY, None)))).await.into_status_and_body();
        let good_id = good["id"].as_str().unwrap().to_owned();
        verify_fork(&router, &good, [&f_token, &g_token]).await;
        // Race: two independent comparators at once yield exactly one supersession.
        let (first, second) = tokio::join!(
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{good_id}/comparisons"),
                    Some(&f_token),
                    Some(comparison(&good, &p, 3, 4, "clean", "2"))
                )
            ),
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{good_id}/comparisons"),
                    Some(&g_token),
                    Some(comparison(&good, &p, 3, 4, "clean", "2"))
                )
            )
        );
        assert_eq!(first.0, StatusCode::CREATED, "{}", first.2);
        assert_eq!(second.0, StatusCode::CREATED, "{}", second.2);
        let transitions = [
            first.1["transition"].as_str().unwrap(),
            second.1["transition"].as_str().unwrap(),
        ];
        assert!(
            transitions.contains(&"superseded") && transitions.contains(&"none"),
            "{transitions:?}"
        );
        assert_eq!(
            store.events(Some("skill.superseded")).await.unwrap().len(),
            1
        );
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{p_id}/lineage"), None, None)
            )
            .await
            .1["active_id"],
            serde_json::json!(good_id)
        );
        // Safety wins over evolution: once the successor is deprecated it is never active, and the
        // still-verified parent is what a latest-active lookup resolves to.
        let (_, gone) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{good_id}/evaluations"),
                Some(&g_token),
                Some(receipt(&good, 3, 4, "leaked", "2")),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(gone["transition"], "deprecated:safety_evaluation_failed");
        let lineage = send(
            &router,
            request("GET", &format!("/v1/skills/{p_id}/lineage"), None, None),
        )
        .await
        .1;
        assert_eq!(lineage["active_id"], serde_json::json!(p_id));
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/v1/skills/{p_id}"), None, None)
            )
            .await
            .1["superseded_by"],
            serde_json::Value::Null,
            "a deprecated successor releases its parent"
        );
        let released = store
            .events(Some("skill.supersession_released"))
            .await
            .unwrap();
        assert_eq!(
            released.len(),
            1,
            "the history stays on record: {released:?}"
        );
        assert_eq!(released[0]["skill_id"], serde_json::json!(p_id));
        let listed = send(
            &router,
            request(
                "GET",
                &format!("/v1/skills/{good_id}/comparisons"),
                None,
                None,
            ),
        )
        .await
        .1;
        assert_eq!(
            listed.as_array().unwrap().len(),
            2,
            "the two racing comparisons"
        );
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c.get("result").is_none())
        );
    }

    #[tokio::test]
    async fn file_backed_store_persists_principals_and_skills_across_restarts() {
        let data_dir = tempfile::tempdir().unwrap();
        let store = RegistryStore::open(data_dir.path()).await.unwrap();
        let (alice, token) = store
            .create_principal("Alice", "org", "team")
            .await
            .unwrap();
        let (skill, created) = store
            .publish(
                &alice,
                &serde_json::from_value(publication(LESSON, SkillVisibility::Team)).unwrap(),
            )
            .await
            .unwrap();
        assert!(created);
        drop(store);
        assert!(data_dir.path().join("registry.db").exists());
        let reopened = RegistryStore::open(data_dir.path()).await.unwrap();
        assert_eq!(
            reopened.authenticate(&token).await.unwrap().unwrap().id,
            alice.id
        );
        let viewer = Viewer {
            principal: Some(alice.clone()),
        };
        assert_eq!(
            reopened
                .get_skill(&viewer, &skill.id)
                .await
                .unwrap()
                .content_digest,
            skill.content_digest
        );
        // The database never holds the token itself, only its digest.
        let raw = std::fs::read(data_dir.path().join("registry.db")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains(&token));
        // A non-loopback bind is refused without an acknowledged TLS proxy.
        let config = RegistryConfig {
            bind: "0.0.0.0:18790".parse().unwrap(),
            data_dir: data_dir.path().into(),
            behind_tls_proxy: false,
            insecure_cookies: false,
        };
        assert!(config.validate().is_err());
        // Loopback development cookies are refused off loopback and behind a TLS proxy.
        assert!(
            RegistryConfig {
                behind_tls_proxy: true,
                insecure_cookies: true,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            RegistryConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                behind_tls_proxy: true,
                insecure_cookies: true,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            RegistryConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                insecure_cookies: true,
                ..config.clone()
            }
            .validate()
            .is_ok()
        );
        // A web session id never reaches the database in the clear.
        let (session, _) = reopened.create_web_session(&alice).await.unwrap();
        assert_eq!(
            reopened
                .authenticate_web_session(&session)
                .await
                .unwrap()
                .unwrap()
                .id,
            alice.id
        );
        let mut raw = Vec::new();
        for name in ["registry.db", "registry.db-wal", "registry.db-shm"] {
            let path = data_dir.path().join(name);
            if path.is_file() {
                raw.extend(std::fs::read(&path).unwrap());
            }
        }
        assert!(
            !String::from_utf8_lossy(&raw).contains(&session),
            "the session id must not appear in the database or its WAL"
        );
        // The data directory and database are private to the service user.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let private = tempfile::tempdir().unwrap();
            let created = private.path().join("nested").join("registry");
            let _ = RegistryStore::open(&created).await.unwrap();
            assert_eq!(
                std::fs::metadata(&created).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(created.join("registry.db"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(
            RegistryConfig {
                behind_tls_proxy: true,
                ..config.clone()
            }
            .validate()
            .is_ok()
        );
        assert!(
            RegistryConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                ..config
            }
            .validate()
            .is_ok()
        );
        // The ephemeral server answers over a real socket.
        let running = start_ephemeral().await.unwrap();
        let health: serde_json::Value = reqwest::get(format!("{}/health", running.url()))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["service"], "s-code-skill-registry");
        running.stop().await;
    }

    #[tokio::test]
    async fn shop_pages_render_escaped_content_and_respect_visibility() {
        let (store, router) = registry().await;
        let (_, alice_token) = principal(&store, "Alice <b>", "team").await;
        let (_, bob_token) = principal(&store, "Bob", "team").await;
        let (_, carol_token) = principal(&store, "Carol", "team").await;
        let hostile =
            "Use <b>bold</b> & <script>alert(1)</script> markers carefully in diagnostics.";
        let (status, skill, _) = send(
            &router,
            request(
                "POST",
                "/v1/skills",
                Some(&alice_token),
                Some(publication(hostile, SkillVisibility::Public)),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{skill}");
        let id = skill["id"].as_str().unwrap().to_owned();
        // Candidate: hidden from the anonymous catalog and detail page, visible to the team.
        let (status, _, html) = send(&router, request("GET", "/shop?status=any", None, None)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!html.contains(&id));
        assert_eq!(
            send(
                &router,
                request("GET", &format!("/shop/skills/{id}"), None, None)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let (_, _, html) = send(
            &router,
            request("GET", "/shop?status=any", Some(&bob_token), None),
        )
        .await;
        assert!(html.contains(&id));
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{html}"
        );
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("Alice &lt;b&gt;"));
        for token in [&bob_token, &carol_token] {
            send(
                &router,
                request(
                    "POST",
                    &format!("/v1/skills/{id}/evaluations"),
                    Some(token),
                    Some(receipt(&skill, 3, 4, "clean", "1")),
                ),
            )
            .await;
        }
        // Verified public: the anonymous catalog lists it, the detail page shows evidence and the notice.
        let (_, _, html) = send(&router, request("GET", "/shop", None, None)).await;
        assert!(html.contains(&id) && html.contains("verified"));
        let (status, _, html) = send(
            &router,
            request("GET", &format!("/shop/skills/{id}"), None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(TRUST_NOTICE));
        assert!(html.contains("Independent evaluators</dt><dd>2"));
        assert!(html.contains("<th>Authority</th>") && html.contains("<td>counts</td>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains(&alice_token) && !html.contains(&bob_token));
        // Forum sections: a hostile challenge and fork render as text; lineage and supersession show.
        let (_, tommy_token) = principal(&store, "Tommy", "team").await;
        let hostile_claim =
            "Fails when <script>alert(2)</script> callers embed the tool & parse \"stderr\".";
        let (status, opened) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/challenges"),
                Some(&tommy_token),
                Some(challenge("applicability_failure", hostile_claim, None)),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{opened}");
        let hostile_lesson = "Use <script>alert(3)</script> and <b>bold</b> markers carefully in diagnostics and exit non-zero.";
        let (status, fork) = send(
            &router,
            request(
                "POST",
                &format!("/v1/skills/{id}/forks"),
                Some(&tommy_token),
                Some(fork_body(
                    hostile_lesson,
                    APPLICABILITY,
                    opened["id"].as_str(),
                )),
            ),
        )
        .await
        .into_status_and_body();
        assert_eq!(status, StatusCode::CREATED, "{fork}");
        let fork_id = fork["id"].as_str().unwrap().to_owned();
        let (_, _, html) = send(
            &router,
            request("GET", &format!("/shop/skills/{id}"), Some(&bob_token), None),
        )
        .await;
        assert!(
            html.contains("<h2>Lineage</h2>")
                && html.contains("<h2>Challenges</h2>")
                && html.contains("<h2>Forks</h2>"),
            "{html}"
        );
        assert!(
            html.contains("&lt;script&gt;alert(2)&lt;/script&gt;")
                && !html.contains("<script>alert(2)")
        );
        assert!(
            html.contains("claim only")
                && html.contains("this version is the active version of its chain")
        );
        assert!(
            html.contains(&fork_id) && html.contains("(v2, candidate)"),
            "{html}"
        );
        let (_, _, filtered) = send(
            &router,
            request(
                "GET",
                &format!("/shop/skills/{id}?challenges=evidence"),
                Some(&bob_token),
                None,
            ),
        )
        .await;
        assert!(filtered.contains("No challenges match.") && !filtered.contains("alert(2)"));
        let (_, _, verified_forks) = send(
            &router,
            request(
                "GET",
                &format!("/shop/skills/{id}?forks=verified"),
                Some(&bob_token),
                None,
            ),
        )
        .await;
        assert!(verified_forks.contains("No forks match."));
        let (status, _, fork_page) = send(
            &router,
            request(
                "GET",
                &format!("/shop/skills/{fork_id}"),
                Some(&bob_token),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            fork_page.contains("<h2>Comparisons against the parent</h2>")
                && fork_page.contains("No comparisons yet.")
        );
        assert!(
            fork_page.contains("&lt;script&gt;alert(3)&lt;/script&gt;")
                && !fork_page.contains("<script>alert(3)")
                && !fork_page.contains("<b>bold</b>")
        );
        assert!(
            !fork_page.contains(&format!("supersedes {id}"))
                && fork_page.contains("no version of its chain is currently active")
        );
        // Anonymous readers see the public skill's lineage without its candidate fork.
        let (_, _, anonymous) = send(
            &router,
            request("GET", &format!("/shop/skills/{id}"), None, None),
        )
        .await;
        assert!(anonymous.contains("<h2>Lineage</h2>") && !anonymous.contains(&fork_id));
        let (_, _, html) = send(&router, request("GET", "/shop/how-to-use", None, None)).await;
        assert!(html.contains("credential_handle"));
        // Login exchanges the token once for an opaque server-side session; the cookie never
        // carries the token, is HttpOnly, Secure, SameSite=Strict, site-wide and bounded.
        let post_form = |router: &Router,
                         uri: &'static str,
                         body: String,
                         extra: Vec<(&'static str, String)>| {
            let mut builder = Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::HOST, "shop.example");
            for (name, value) in extra {
                builder = builder.header(name, value);
            }
            router
                .clone()
                .oneshot(builder.body(Body::from(body)).unwrap())
        };
        let response = post_form(&router, "/shop/login", format!("token={bob_token}"), vec![])
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let session = cookie.split(';').next().unwrap().to_owned();
        assert!(session.starts_with("registry_session=wsess_"), "{cookie}");
        assert!(
            !cookie.contains(&bob_token) && !cookie.contains("skr_"),
            "the token never enters a cookie: {cookie}"
        );
        for attribute in [
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
            "Path=/",
            "Max-Age=43200",
        ] {
            assert!(
                cookie.contains(attribute),
                "{attribute} missing from {cookie}"
            );
        }
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_some()
        );
        // A bogus token yields no cookie; a cross-site form post is refused before any check.
        let response = post_form(&router, "/shop/login", "token=skr_bogus".into(), vec![])
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![("origin", "https://evil.example".into())],
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![("sec-fetch-site", "cross-site".into())],
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![
                ("origin", "https://shop.example".into()),
                ("sec-fetch-site", "same-origin".into()),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "same-origin posts are accepted"
        );
        // A browser's own form post under a strict referrer policy carries `Origin: null` with
        // `Sec-Fetch-Site: same-origin`: Fetch Metadata decides, so it is accepted.
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![
                ("origin", "null".into()),
                ("sec-fetch-site", "same-origin".into()),
            ],
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "browser form post with a null origin"
        );
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![("origin", "null".into())],
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "an opaque origin without Fetch Metadata is refused"
        );
        // Logging in while presenting a live session retires that session.
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={bob_token}"),
            vec![("cookie", session.clone())],
        )
        .await
        .unwrap();
        let rotated = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_ne!(rotated, session);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/shop?status=any")
                    .header(header::COOKIE, session.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let html =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(
            !html.contains("Signed in as"),
            "the previous session is dead after re-login"
        );
        let session = rotated;
        // The session authenticates the browser; pages carry hardening headers and no token.
        let get_with = |router: &Router, uri: &'static str, cookie: String| {
            router.clone().oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let response = get_with(&router, "/shop?status=any", session.clone())
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let html =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(html.contains("Signed in as Bob"));
        assert!(!html.contains(&bob_token) && !html.contains("wsess_"));
        // A forged or copied-but-unknown session is anonymous.
        let response = get_with(
            &router,
            "/shop?status=any",
            "registry_session=wsess_forged".into(),
        )
        .await
        .unwrap();
        let html =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(
            !html.contains("Signed in as"),
            "a forged session is anonymous"
        );
        // Logout revokes the server-side session: the copied cookie is dead afterwards.
        let response = post_form(
            &router,
            "/shop/logout",
            String::new(),
            vec![("cookie", session.clone())],
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let cleared = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            cleared.starts_with("registry_session=;") && cleared.contains("Max-Age=0"),
            "{cleared}"
        );
        let response = get_with(&router, "/shop?status=any", session.clone())
            .await
            .unwrap();
        let html =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(
            !html.contains("Signed in as"),
            "a revoked session never authenticates again"
        );
        assert_eq!(
            store
                .events(Some("web_session.revoked"))
                .await
                .unwrap()
                .len(),
            2,
            "one revocation from the re-login rotation, one from logout"
        );
        // A disabled principal's live session stops working on the next request.
        let response = post_form(
            &router,
            "/shop/login",
            format!("token={carol_token}"),
            vec![],
        )
        .await
        .unwrap();
        let carol_session = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let carol_id = store.authenticate(&carol_token).await.unwrap().unwrap().id;
        store.disable_principal(&carol_id).await.unwrap();
        let response = get_with(&router, "/shop?status=any", carol_session)
            .await
            .unwrap();
        let html =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(!html.contains("Signed in as"));
        // The registry's own records hold session ids and digests, never tokens or sessions in events.
        let events = serde_json::to_string(&store.events(None).await.unwrap()).unwrap();
        assert!(!events.contains(&bob_token) && !events.contains("wsess_"));
        // Loopback development mode omits Secure and nothing else; production keeps it.
        let development = app_with(
            store.clone(),
            WebSettings {
                insecure_cookies: true,
            },
        );
        let response = post_form(
            &development,
            "/shop/login",
            format!("token={bob_token}"),
            vec![],
        )
        .await
        .unwrap();
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(
            !cookie.contains("Secure")
                && cookie.contains("HttpOnly")
                && cookie.contains("SameSite=Strict"),
            "{cookie}"
        );
        assert!(!cookie.contains(&bob_token));
    }

    // -- forum hardening --------------------------------------------------

    fn comparison_with(
        fork: &serde_json::Value,
        parent: &serde_json::Value,
        passes: (u32, u32),
        safety: (&str, &str),
        family: Option<&str>,
        version: &str,
    ) -> serde_json::Value {
        let (parent_safety, fork_safety) = safety;
        let mut body = comparison(fork, parent, passes.0, passes.1, fork_safety, version);
        body["parent_safety"] = serde_json::json!({"verdict": parent_safety, "candidate_retrieved_only_skill": parent_safety == "clean", "harmful_rule_absent_from_requests": parent_safety == "clean"});
        body["task_family"] = serde_json::json!(family);
        body
    }

    async fn post(
        router: &Router,
        uri: &str,
        token: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        send(router, request("POST", uri, Some(token), Some(body)))
            .await
            .into_status_and_body()
    }

    async fn get(router: &Router, uri: &str, token: Option<&str>) -> serde_json::Value {
        send(router, request("GET", uri, token, None)).await.1
    }

    async fn fork_of(
        router: &Router,
        parent: &serde_json::Value,
        forker: &str,
        lesson: &str,
    ) -> serde_json::Value {
        let (status, fork) = post(
            router,
            &format!("/v1/skills/{}/forks", parent["id"].as_str().unwrap()),
            forker,
            fork_body(lesson, APPLICABILITY, None),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{fork}");
        fork
    }

    async fn compare(
        router: &Router,
        fork: &serde_json::Value,
        token: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let (status, accepted) = post(
            router,
            &format!("/v1/skills/{}/comparisons", fork["id"].as_str().unwrap()),
            token,
            body,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{accepted}");
        accepted
    }

    async fn active_of(router: &Router, id: &str) -> serde_json::Value {
        get(router, &format!("/v1/skills/{id}/lineage"), None).await["active_id"].clone()
    }

    #[tokio::test]
    async fn lineage_authority_needs_one_team_or_authorized_evaluators() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let (_, e1) = store
            .create_principal_with("E1", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e2) = store
            .create_principal_with("E2", "org", "eval-team", true)
            .await
            .unwrap();
        let s1 = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let s1_id = s1["id"].as_str().unwrap();
        let family = Some("cli-error-contract");
        let compare_all =
            |fork: serde_json::Value, parent: serde_json::Value, tokens: Vec<String>| {
                let router = router.clone();
                async move {
                    let mut last = serde_json::Value::Null;
                    for token in tokens {
                        last = compare(
                            &router,
                            &fork,
                            &token,
                            comparison(&fork, &parent, 3, 5, "clean", "1"),
                        )
                        .await;
                    }
                    last
                }
            };
        // Another team forks the public skill and verifies its own fork; its
        // comparisons, and the parent team's, do not decide a link between
        // two teams' skills.
        let s2 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &s2, [&m2, &m3]).await;
        for token in [&m2, &m3, &bob, &carol] {
            let accepted = compare(
                &router,
                &s2,
                token,
                comparison(&s2, &s1, 3, 5, "clean", "1"),
            )
            .await;
            assert_eq!(accepted["comparison"]["authoritative"], false, "{accepted}");
            assert_eq!(accepted["transition"], "none");
        }
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        // Authorized evaluators may adopt it.
        let adopted = compare_all(s2.clone(), s1.clone(), vec![e1.clone(), e2.clone()]).await;
        assert_eq!(adopted["transition"], "superseded", "{adopted}");
        assert_eq!(active_of(&router, s1_id).await, s2["id"]);
        // The adopted fork's team cannot extend the chain that now answers
        // for team A: its link authority is gone, and so is team A's alone.
        let s2x = fork_of(
            &router,
            &s2,
            &m1,
            "Return a distinct exit status for malformed input and name the field twice.",
        )
        .await;
        verify_fork(&router, &s2x, [&m2, &m3]).await;
        let takeover = compare_all(s2x.clone(), s2.clone(), vec![m2.clone(), m3.clone()]).await;
        assert_eq!(takeover["comparison"]["authoritative"], false, "{takeover}");
        assert_eq!(takeover["transition"], "none");
        let alone = compare_all(s2x.clone(), s2.clone(), vec![bob.clone(), carol.clone()]).await;
        assert_eq!(alone["transition"], "none", "{alone}");
        assert_eq!(
            active_of(&router, s1_id).await,
            s2["id"],
            "no transitive takeover"
        );
        let extended = compare_all(s2x.clone(), s2.clone(), vec![e1.clone(), e2.clone()]).await;
        assert_eq!(extended["transition"], "superseded", "{extended}");
        assert_eq!(active_of(&router, s1_id).await, s2x["id"]);
        // The parent's owners withdraw their skill's link with safety
        // evidence; the fork's own chain stays as it is.
        let vetoed = compare(
            &router,
            &s2,
            &bob,
            comparison_with(&s2, &s1, (3, 5), ("clean", "leaked"), family, "2"),
        )
        .await;
        assert_eq!(vetoed["comparison"]["parent_authority"], true, "{vetoed}");
        assert_eq!(vetoed["transition"], "withdrawn");
        assert_eq!(
            vetoed["fork"]["status"], "verified",
            "only the fork's team or an evaluator deprecates it"
        );
        assert_eq!(vetoed["parent"].get("superseded_by"), None, "{vetoed}");
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        assert_eq!(
            active_of(&router, s2["id"].as_str().unwrap()).await,
            s2x["id"]
        );
        // A fork that already has a successor is never adopted with its chain.
        let t2 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and name the reason.",
        )
        .await;
        verify_fork(&router, &t2, [&m2, &m3]).await;
        let t3 = fork_of(
            &router,
            &t2,
            &m1,
            "Return a distinct exit status for malformed input and name the reason twice.",
        )
        .await;
        verify_fork(&router, &t3, [&m2, &m3]).await;
        let own_chain = compare_all(t3.clone(), t2.clone(), vec![m2.clone(), m3.clone()]).await;
        assert_eq!(
            own_chain["comparison"]["authoritative"], true,
            "a team decides inside its own chain"
        );
        assert_eq!(own_chain["transition"], "superseded", "{own_chain}");
        let refused = compare_all(t2.clone(), s1.clone(), vec![e1.clone(), e2.clone()]).await;
        assert_eq!(refused["transition"], "none", "{refused}");
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        // Inside a chain of its own skills, a team decides alone.
        let a2 = fork_of(
            &router,
            &s1,
            &alice,
            "Return a distinct exit status for malformed input and log it.",
        )
        .await;
        verify_fork(&router, &a2, [&bob, &carol]).await;
        let own = compare_all(a2.clone(), s1.clone(), vec![bob.clone(), carol.clone()]).await;
        assert_eq!(own["transition"], "superseded", "{own}");
        assert_eq!(active_of(&router, s1_id).await, a2["id"]);
        // A parent arm that leaks deprecates the parent and releases its
        // predecessor.
        let a3 = fork_of(
            &router,
            &a2,
            &alice,
            "Return a distinct exit status for malformed input and log it twice.",
        )
        .await;
        let leaked = compare(
            &router,
            &a3,
            &e1,
            comparison_with(&a3, &a2, (3, 5), ("leaked", "clean"), family, "1"),
        )
        .await;
        assert_eq!(leaked["parent"]["status"], "deprecated", "{leaked}");
        assert_eq!(
            get(&router, &format!("/v1/skills/{s1_id}"), None)
                .await
                .get("superseded_by"),
            None
        );
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
    }

    #[tokio::test]
    async fn safety_evidence_in_comparisons_blocks_or_deprecates_by_authority() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let (_, e1) = store
            .create_principal_with("E1", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e2) = store
            .create_principal_with("E2", "org", "eval-team", true)
            .await
            .unwrap();
        let s1 = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let s1_id = s1["id"].as_str().unwrap();
        let family = Some("cli-error-contract");
        // The parent's team reports that another team's fork leaked: the fork is
        // blocked from superseding for good, but only its own team or an
        // authorized evaluator can deprecate it.
        let s2 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &s2, [&m2, &m3]).await;
        let blocked = compare(
            &router,
            &s2,
            &bob,
            comparison_with(&s2, &s1, (3, 5), ("clean", "leaked"), family, "1"),
        )
        .await;
        assert_eq!(blocked["comparison"]["authoritative"], false);
        assert_eq!(blocked["comparison"]["parent_authority"], true);
        assert_eq!(blocked["fork"]["status"], "verified", "{blocked}");
        for token in [&e1, &e2] {
            assert_eq!(
                compare(
                    &router,
                    &s2,
                    token,
                    comparison_with(&s2, &s1, (3, 5), ("clean", "clean"), family, "1")
                )
                .await["transition"],
                "none",
                "one authoritative safety failure blocks for good"
            );
        }
        // An authorized evaluator's report deprecates the fork.
        let s3 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and log the field.",
        )
        .await;
        verify_fork(&router, &s3, [&m2, &m3]).await;
        let deprecated = compare(
            &router,
            &s3,
            &e1,
            comparison_with(&s3, &s1, (3, 5), ("clean", "leaked"), family, "1"),
        )
        .await;
        assert_eq!(deprecated["fork"]["status"], "deprecated", "{deprecated}");
        assert_eq!(
            deprecated["fork"]["deprecation_reason"],
            "comparison_safety_failed"
        );
        let (status, _) = post(
            &router,
            &format!("/v1/skills/{}/comparisons", s3["id"].as_str().unwrap()),
            &e2,
            comparison_with(&s3, &s1, (3, 5), ("clean", "clean"), family, "1"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a deprecated fork is never compared"
        );
        // A failed parent arm deprecates the parent.
        let s4 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and print the field.",
        )
        .await;
        verify_fork(&router, &s4, [&m2, &m3]).await;
        let parent_failed = compare(
            &router,
            &s4,
            &e2,
            comparison_with(&s4, &s1, (3, 5), ("leaked", "clean"), family, "1"),
        )
        .await;
        assert_eq!(
            parent_failed["parent"]["status"], "deprecated",
            "{parent_failed}"
        );
        assert_eq!(active_of(&router, s1_id).await, serde_json::Value::Null);
        let (status, _) = post(
            &router,
            &format!("/v1/skills/{}/comparisons", s4["id"].as_str().unwrap()),
            &e1,
            comparison_with(&s4, &s1, (3, 5), ("clean", "clean"), family, "1"),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a deprecated parent is never compared"
        );
    }

    #[tokio::test]
    async fn supersession_settles_on_late_verification_and_when_a_successor_falls() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, dave) = principal(&store, "Dave", "team-a").await;
        let (_, erin) = principal(&store, "Erin", "team-a").await;
        let (_, e1) = store
            .create_principal_with("E1", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e2) = store
            .create_principal_with("E2", "org", "eval-team", true)
            .await
            .unwrap();
        let s1 = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let s1_id = s1["id"].as_str().unwrap();
        // Compared while still a candidate: nothing yet.
        let s2 = fork_of(
            &router,
            &s1,
            &dave,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        for token in [&e1, &e2] {
            assert_eq!(
                compare(
                    &router,
                    &s2,
                    token,
                    comparison(&s2, &s1, 3, 5, "clean", "1")
                )
                .await["transition"],
                "none"
            );
        }
        // Its verification settles the gate with the comparisons already recorded.
        verify_fork(&router, &s2, [&bob, &carol]).await;
        assert_eq!(
            get(&router, &format!("/v1/skills/{s1_id}"), None).await["superseded_by"],
            s2["id"]
        );
        assert_eq!(
            store.events(Some("skill.superseded")).await.unwrap().len(),
            1
        );
        // A later, qualifying fork answering a challenge waits while the
        // successor stands ...
        let (status, filed) = post(
            &router,
            &format!("/v1/skills/{s1_id}/challenges"),
            &dave,
            challenge(
                "correctness_failure",
                "The rule misses callers that ignore the status.",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{filed}");
        let challenge_id = filed["id"].as_str().unwrap().to_owned();
        let (status, s3) = post(
            &router,
            &format!("/v1/skills/{s1_id}/forks"),
            &erin,
            fork_body(
                "Return a distinct exit status for malformed input and log the field.",
                APPLICABILITY,
                Some(&challenge_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{s3}");
        verify_fork(&router, &s3, [&bob, &carol]).await;
        for token in [&e1, &e2] {
            assert_eq!(
                compare(
                    &router,
                    &s3,
                    token,
                    comparison(&s3, &s1, 3, 4, "clean", "1")
                )
                .await["transition"],
                "none"
            );
        }
        // ... and replaces it the moment the successor is deprecated.
        let (status, _) = post(
            &router,
            &format!("/v1/skills/{}/deprecate", s2["id"].as_str().unwrap()),
            &dave,
            serde_json::json!({"reason": "withdrawn"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            get(&router, &format!("/v1/skills/{s1_id}"), None).await["superseded_by"],
            s3["id"]
        );
        for id in [s1_id, s3["id"].as_str().unwrap()] {
            assert_eq!(
                active_of(&router, id).await,
                s3["id"],
                "every member of the chain resolves to the same version"
            );
        }
        let events = store.events(Some("skill.superseded")).await.unwrap();
        assert_eq!(events.len(), 2);
        let challenges = get(&router, &format!("/v1/skills/{s1_id}/challenges"), None).await;
        assert_eq!(challenges[0]["status"], "addressed");
        // A link holds only while its gate holds: an evaluator's newer
        // comparison in which the fork regresses withdraws it, and the
        // challenge it answered is open again.
        let withdrawn = compare(&router, &s3, &e1, comparison(&s3, &s1, 4, 2, "clean", "2")).await;
        assert_eq!(withdrawn["transition"], "withdrawn");
        assert_eq!(
            withdrawn["parent"].get("superseded_by"),
            None,
            "{withdrawn}"
        );
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        let challenges = get(&router, &format!("/v1/skills/{s1_id}/challenges"), None).await;
        assert_eq!(challenges[0]["status"], "open", "{challenges}");
        assert_eq!(
            challenges[0]["addressed_by_skill_id"],
            serde_json::Value::Null
        );
        let released = store
            .events(Some("skill.supersession_released"))
            .await
            .unwrap();
        assert_eq!(
            released.last().unwrap()["payload"]["reason"],
            "gate_no_longer_holds"
        );
        // One principal records at most ten comparisons of one fork, and the
        // comparisons are read page by page.
        for version in 2..=10 {
            compare(
                &router,
                &s3,
                &e2,
                comparison(&s3, &s1, 3, 4, "clean", &version.to_string()),
            )
            .await;
        }
        let (status, capped) = post(
            &router,
            &format!("/v1/skills/{}/comparisons", s3["id"].as_str().unwrap()),
            &e2,
            comparison(&s3, &s1, 3, 4, "clean", "11"),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{capped}");
        let uri = format!("/v1/skills/{}/comparisons", s3["id"].as_str().unwrap());
        let first = get(&router, &format!("{uri}?limit=8"), None).await;
        let last_id = first[7]["id"].as_str().unwrap();
        let rest = get(&router, &format!("{uri}?limit=8&before={last_id}"), None).await;
        assert_eq!(
            first.as_array().unwrap().len() + rest.as_array().unwrap().len(),
            12
        );
    }

    #[tokio::test]
    async fn task_family_falls_back_to_the_parent_and_is_required() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, dave) = principal(&store, "Dave", "team-a").await;
        let (_, e1) = store
            .create_principal_with("E1", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e2) = store
            .create_principal_with("E2", "org", "eval-team", true)
            .await
            .unwrap();
        let unfamilied_fork = |lesson: &str| {
            let mut body = fork_body(lesson, APPLICABILITY, None);
            body["provenance"] = serde_json::json!({"source_kind": "refinement"});
            body
        };
        // The parent declares a family; the fork does not: the parent's applies.
        let p1 = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let (_, f1) = post(
            &router,
            &format!("/v1/skills/{}/forks", p1["id"].as_str().unwrap()),
            &dave,
            unfamilied_fork(
                "Return a distinct exit status for malformed input and name the field.",
            ),
        )
        .await;
        verify_fork(&router, &f1, [&bob, &carol]).await;
        for token in [&e1, &e2] {
            assert_eq!(
                compare(
                    &router,
                    &f1,
                    token,
                    comparison_with(
                        &f1,
                        &p1,
                        (3, 5),
                        ("clean", "clean"),
                        Some("unrelated-family"),
                        "1"
                    )
                )
                .await["transition"],
                "none"
            );
        }
        assert_eq!(
            compare(
                &router,
                &f1,
                &e1,
                comparison_with(
                    &f1,
                    &p1,
                    (3, 5),
                    ("clean", "clean"),
                    Some("cli-error-contract"),
                    "2"
                )
            )
            .await["transition"],
            "none"
        );
        assert_eq!(
            compare(
                &router,
                &f1,
                &e2,
                comparison_with(
                    &f1,
                    &p1,
                    (3, 5),
                    ("clean", "clean"),
                    Some("cli-error-contract"),
                    "2"
                )
            )
            .await["transition"],
            "superseded",
            "the newest comparison of each evaluator counts"
        );
        // Neither declares a family: nothing is comparable.
        let mut unfamilied = publication(
            "Prefer explicit exit codes when scripts are composed.",
            SkillVisibility::Public,
        );
        unfamilied["provenance"] = serde_json::json!({"source_kind": "distilled"});
        let (_, p2) = post(&router, "/v1/skills", &alice, unfamilied).await;
        verify_fork(&router, &p2, [&bob, &carol]).await;
        let p2 = get(
            &router,
            &format!("/v1/skills/{}", p2["id"].as_str().unwrap()),
            Some(&alice),
        )
        .await;
        let (_, f2) = post(
            &router,
            &format!("/v1/skills/{}/forks", p2["id"].as_str().unwrap()),
            &dave,
            unfamilied_fork("Prefer explicit exit codes when scripts are composed and piped."),
        )
        .await;
        verify_fork(&router, &f2, [&bob, &carol]).await;
        for token in [&e1, &e2] {
            assert_eq!(
                compare(
                    &router,
                    &f2,
                    token,
                    comparison_with(
                        &f2,
                        &p2,
                        (3, 5),
                        ("clean", "clean"),
                        Some("cli-error-contract"),
                        "1"
                    )
                )
                .await["transition"],
                "none"
            );
        }
    }

    #[tokio::test]
    async fn legacy_declared_parents_are_not_lineage_after_migration() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use std::str::FromStr;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = dir.join("registry.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.migrations = std::borrow::Cow::Owned(
            migrator
                .migrations
                .iter()
                .filter(|m| m.version <= 4)
                .cloned()
                .collect(),
        );
        migrator.run(&pool).await.unwrap();
        let now = "2026-09-01T00:00:00+00:00";
        for (id, team) in [("p_alice", "team-a"), ("p_bob", "team-b")] {
            sqlx::query("INSERT INTO principals (id, display_name, organization_id, team_id, token_hash, disabled, created_at, authorized_evaluator) VALUES (?,?,?,?,?,0,?,0)")
                .bind(id).bind(id).bind("org").bind(team).bind(format!("hash-{id}")).bind(now)
                .execute(&pool).await.unwrap();
        }
        for (id, team, publisher, visibility, parent, version, lesson) in [
            (
                "skill_parent",
                "team-a",
                "p_alice",
                "public",
                None,
                1_i64,
                "Return a distinct exit status for malformed input.",
            ),
            (
                "skill_claimed_child",
                "team-b",
                "p_bob",
                "team",
                Some("skill_parent"),
                7,
                "Return a distinct exit status for malformed input and name it.",
            ),
        ] {
            sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, deprecation_reason, parent_skill_id, version, provenance_json, created_at, updated_at, verified_at, deprecated_at) VALUES (?,?,?,?,?,?,?,?,?,1,'verified',NULL,?,?,?,?,?,?,NULL)")
                .bind(id).bind("org").bind(team).bind(publisher).bind(publisher).bind(visibility)
                .bind(lesson).bind(APPLICABILITY).bind(skill_content_digest(lesson, APPLICABILITY, 1))
                .bind(parent).bind(version).bind(r#"{"source_kind":"distilled","task_family":"cli-error-contract"}"#)
                .bind(now).bind(now).bind(now)
                .execute(&pool).await.unwrap();
        }
        pool.close().await;
        let store = RegistryStore::open(&dir).await.unwrap();
        let bob = store.get_principal("p_bob").await.unwrap().unwrap();
        let bob_view = Viewer {
            principal: Some(bob.clone()),
        };
        let child = store
            .get_skill(&bob_view, "skill_claimed_child")
            .await
            .unwrap();
        assert_eq!(
            child.parent_skill_id, None,
            "a claimed parent is not a fork"
        );
        let lineage = store.lineage(&bob_view, "skill_parent").await.unwrap();
        assert_eq!(lineage.nodes.len(), 1);
        let parent = store.get_skill(&bob_view, "skill_parent").await.unwrap();
        let comparison_body: s_code_skill_shop::ComparisonSubmission =
            serde_json::from_value(comparison(
                &serde_json::to_value(&child).unwrap(),
                &serde_json::to_value(&parent).unwrap(),
                3,
                5,
                "clean",
                "1",
            ))
            .unwrap();
        assert!(matches!(
            store
                .record_comparison(&bob, "skill_claimed_child", &comparison_body)
                .await,
            Err(StoreError::Invalid(_))
        ));
        let reader = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display())).unwrap(),
            )
            .await
            .unwrap();
        let (root, depth, claimed): (String, i64, Option<String>) = sqlx::query_as("SELECT lineage_root_id, lineage_depth, legacy_declared_parent_id FROM skills WHERE id='skill_claimed_child'")
            .fetch_one(&reader).await.unwrap();
        assert_eq!(
            (root.as_str(), depth, claimed.as_deref()),
            ("skill_claimed_child", 0, Some("skill_parent")),
            "the claim is kept for the record only"
        );
        reader.close().await;
    }

    #[tokio::test]
    async fn supersession_authority_migration_backfills_and_releases_deprecated_successors() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
        use std::str::FromStr;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = dir.join("registry.db");
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .unwrap()
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.migrations = std::borrow::Cow::Owned(
            migrator
                .migrations
                .iter()
                .filter(|m| m.version <= 9)
                .cloned()
                .collect(),
        );
        migrator.run(&pool).await.unwrap();
        let now = "2026-09-01T00:00:00+00:00";
        sqlx::query("INSERT INTO principals (id, display_name, organization_id, team_id, token_hash, disabled, created_at, authorized_evaluator) VALUES ('p_alice','p_alice','org','team-a','hash-a',0,?,0)")
            .bind(now).execute(&pool).await.unwrap();
        // A verified parent whose successor was deprecated before links were
        // released, and a comparison that recorded the parent's authority.
        for (id, status, parent, lesson) in [
            (
                "skill_parent",
                "verified",
                None,
                "Return a distinct exit status for malformed input.",
            ),
            (
                "skill_fallen",
                "deprecated",
                Some("skill_parent"),
                "Return a distinct exit status for malformed input and name it.",
            ),
        ] {
            sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, parent_skill_id, version, provenance_json, created_at, updated_at, forked_by_id, forked_by_display, lineage_root_id, lineage_depth) VALUES (?,'org','team-a','p_alice','p_alice','public',?,?,?,1,?,?,?,'{}',?,?,?,?,'skill_parent',?)")
                .bind(id).bind(lesson).bind(APPLICABILITY).bind(skill_content_digest(lesson, APPLICABILITY, 1)).bind(status)
                .bind(parent).bind(if parent.is_some() { 2 } else { 1 }).bind(now).bind(now)
                .bind(parent.map(|_| "p_alice")).bind(parent.map(|_| "p_alice"))
                .bind(i64::from(parent.is_some()))
                .execute(&pool).await.unwrap();
        }
        sqlx::query("UPDATE skills SET superseded_by='skill_fallen', superseded_at=? WHERE id='skill_parent'")
            .bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO challenges (id, skill_id, content_digest, skill_version, challenger_id, challenger_display, kind, claim, applicability, evidence_receipt_id, claim_digest, status, created_at, addressed_at, addressed_by_skill_id) SELECT 'challenge_old', id, content_digest, 1, 'p_alice', 'p_alice', 'correctness_failure', 'The rule misses library callers.', NULL, NULL, ?, 'addressed', ?, ?, 'skill_fallen' FROM skills WHERE id='skill_parent'")
            .bind("cd".repeat(32)).bind(now).bind(now).execute(&pool).await.unwrap();
        // A link another team's fork got from the parent's own team alone,
        // under the older parent-relative authority.
        sqlx::query("INSERT INTO principals (id, display_name, organization_id, team_id, token_hash, disabled, created_at, authorized_evaluator) VALUES ('p_bob','p_bob','org','team-b','hash-b',0,?,0)")
            .bind(now).execute(&pool).await.unwrap();
        for (id, team, publisher, parent, lesson) in [
            (
                "skill_owned",
                "team-a",
                "p_alice",
                None,
                "Keep diagnostics on standard error so pipelines stay parseable.",
            ),
            (
                "skill_taken",
                "team-b",
                "p_bob",
                Some("skill_owned"),
                "Keep diagnostics on standard error so pipelines stay parseable, always.",
            ),
        ] {
            sqlx::query("INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, parent_skill_id, version, provenance_json, created_at, updated_at, forked_by_id, forked_by_display, lineage_root_id, lineage_depth) VALUES (?,'org',?,?,?,'public',?,?,?,1,'verified',?,?,'{\"task_family\":\"cli-error-contract\"}',?,?,?,?,'skill_owned',?)")
                .bind(id).bind(team).bind(publisher).bind(publisher).bind(lesson).bind(APPLICABILITY).bind(skill_content_digest(lesson, APPLICABILITY, 1))
                .bind(parent).bind(if parent.is_some() { 2 } else { 1 }).bind(now).bind(now)
                .bind(parent.map(|_| publisher)).bind(parent.map(|_| publisher))
                .bind(i64::from(parent.is_some()))
                .execute(&pool).await.unwrap();
        }
        sqlx::query(
            "UPDATE skills SET superseded_by='skill_taken', superseded_at=? WHERE id='skill_owned'",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO comparisons (id, fork_id, parent_id, principal_id, principal_display, evaluator_json, independent, authoritative, protocol_version, protocol_digest, complete, fork_safety, task_family, model, verdict_json, result_json, created_at) VALUES ('comparison_old','skill_fallen','skill_parent','p_alice','p_alice','{}',0,1,1,?,1,'clean','cli-error-contract','model-x','{}','{}',?)")
            .bind("ab".repeat(32)).bind(now).execute(&pool).await.unwrap();
        pool.close().await;
        let store = RegistryStore::open(&dir).await.unwrap();
        let alice = store.get_principal("p_alice").await.unwrap().unwrap();
        let view = Viewer {
            principal: Some(alice),
        };
        let parent = store.get_skill(&view, "skill_parent").await.unwrap();
        assert_eq!(
            parent.superseded_by, None,
            "a deprecated successor releases its parent"
        );
        let challenges = store
            .list_challenges(
                &view,
                "skill_parent",
                store::ChallengeFilter::All,
                &store::ForumPage::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            challenges[0].status,
            s_code_skill_shop::ChallengeStatus::Open
        );
        let owned = store.get_skill(&view, "skill_owned").await.unwrap();
        assert_eq!(
            owned.superseded_by, None,
            "a link decided without today's authority is released"
        );
        let released = store
            .events(Some("skill.supersession_released"))
            .await
            .unwrap();
        let reasons: Vec<(&str, &str)> = released
            .iter()
            .map(|event| {
                (
                    event["skill_id"].as_str().unwrap(),
                    event["payload"]["reason"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            reasons,
            vec![
                ("skill_owned", "gate_no_longer_holds"),
                ("skill_parent", "successor_deprecated")
            ]
        );
        assert_eq!(
            store
                .lineage(&view, "skill_parent")
                .await
                .unwrap()
                .active_id
                .as_deref(),
            Some("skill_parent")
        );
        let comparisons = store
            .list_comparisons(&view, "skill_fallen", &store::ForumPage::default())
            .await
            .unwrap();
        assert_eq!(comparisons.len(), 1);
        assert!(
            comparisons[0].parent_authority,
            "the recorded parent authority is kept"
        );
        drop(store);
        let reopened = RegistryStore::open(&dir).await.unwrap();
        assert_eq!(
            reopened
                .events(Some("skill.supersession_released"))
                .await
                .unwrap()
                .len(),
            2,
            "the pass is idempotent"
        );
        // A relabelled copy of a version 1 parent is no refinement.
        let alice = reopened.get_principal("p_alice").await.unwrap().unwrap();
        let lesson = "Return a distinct exit status for malformed input.";
        let relabelled = s_code_skill_shop::ForkSubmission {
            lesson: lesson.into(),
            applicability: APPLICABILITY.into(),
            content_digest: skill_content_digest(lesson, APPLICABILITY, SKILL_SANITIZATION_VERSION),
            sanitization_version: SKILL_SANITIZATION_VERSION,
            provenance: SkillProvenance::default(),
            responding_to_challenge_id: None,
        };
        assert!(matches!(
            reopened
                .create_fork(&alice, "skill_parent", &relabelled)
                .await,
            Err(StoreError::Invalid(_))
        ));
        // A team holds one skill per text, whatever version labels it: the
        // same text as its version 1 fork or skill is a conflict.
        let lesson = "Return a distinct exit status for malformed input and name it.";
        let copied = s_code_skill_shop::ForkSubmission {
            lesson: lesson.into(),
            content_digest: skill_content_digest(lesson, APPLICABILITY, SKILL_SANITIZATION_VERSION),
            ..relabelled
        };
        assert!(matches!(
            reopened.create_fork(&alice, "skill_parent", &copied).await,
            Err(StoreError::Conflict(_))
        ));
        let republished: SkillPublication = serde_json::from_value(publication(
            "Keep diagnostics on standard error so pipelines stay parseable.",
            SkillVisibility::Public,
        ))
        .unwrap();
        assert!(matches!(
            reopened.publish(&alice, &republished).await,
            Err(StoreError::Conflict(_))
        ));
        drop(reopened);
        // Rows written behind the registry's back: a link to a missing
        // skill, a link to a skill that is not a fork of the parent and an
        // unreadable skill. The store still opens, releases those links and
        // serves everything else.
        let raw = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
                    .unwrap()
                    .foreign_keys(false),
            )
            .await
            .unwrap();
        for statement in [
            "PRAGMA ignore_check_constraints = ON",
            "UPDATE skills SET superseded_by='skill_missing' WHERE id='skill_owned'",
            "UPDATE skills SET superseded_by='skill_parent' WHERE id='skill_taken'",
            "INSERT INTO skills (id, organization_id, team_id, publisher_id, publisher_display, visibility, lesson, applicability, content_digest, sanitization_version, status, version, provenance_json, created_at, updated_at, lineage_root_id, lineage_depth) VALUES ('skill_bogus','org','team-a','p_alice','p_alice','public','Unreadable.','Anywhere.','ff',2,'bogus',1,'{}','2026-09-01T00:00:00+00:00','2026-09-01T00:00:00+00:00','skill_bogus',0)",
            "PRAGMA user_version = 0",
        ] {
            sqlx::query(statement).execute(&raw).await.unwrap();
        }
        raw.close().await;
        let store = RegistryStore::open(&dir).await.unwrap();
        for id in ["skill_owned", "skill_taken"] {
            assert_eq!(
                store.get_skill(&view, id).await.unwrap().superseded_by,
                None,
                "{id}"
            );
        }
        let reasons: Vec<String> = store
            .events(Some("skill.supersession_released"))
            .await
            .unwrap()
            .iter()
            .map(|event| event["payload"]["reason"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            reasons.contains(&"successor_missing".to_owned()),
            "{reasons:?}"
        );
        assert!(
            reasons.contains(&"not_a_fork_of_the_parent".to_owned()),
            "{reasons:?}"
        );
        let listed = store
            .list_skills(&view, &ListFilter::default())
            .await
            .unwrap();
        assert!(!listed.iter().any(|skill| skill.id == "skill_bogus"));
        assert!(listed.iter().any(|skill| skill.id == "skill_parent"));
    }

    #[tokio::test]
    async fn released_links_recheck_their_chain_and_requests_use_current_authority() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let (_, m4) = principal(&store, "Mallory4", "team-b").await;
        let mut evaluators = Vec::new();
        for name in ["E1", "E2", "E3"] {
            evaluators.push(
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap(),
            );
        }
        let [e1, e2, e3] = [0, 1, 2].map(|i| evaluators[i].1.clone());
        let family = Some("cli-error-contract");
        let adopt = |fork: serde_json::Value, parent: serde_json::Value, tokens: Vec<String>| {
            let router = router.clone();
            async move {
                let mut last = serde_json::Value::Null;
                for token in tokens {
                    last = compare(
                        &router,
                        &fork,
                        &token,
                        comparison(&fork, &parent, 3, 5, "clean", "1"),
                    )
                    .await;
                }
                last
            }
        };
        // G (team A) → P (team B's fork) → F (team B's fork of P).
        let g = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let g_id = g["id"].as_str().unwrap();
        let p = fork_of(
            &router,
            &g,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &p, [&m2, &m3]).await;
        let p_id = p["id"].as_str().unwrap();
        let f = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the field twice.",
        )
        .await;
        verify_fork(&router, &f, [&m2, &m3]).await;
        // While P and F form team B's own chain, B's regression of F counts.
        let regression = compare(&router, &f, &m4, comparison(&f, &p, 4, 2, "clean", "1")).await;
        assert_eq!(
            regression["comparison"]["authoritative"], true,
            "{regression}"
        );
        assert_eq!(
            adopt(p.clone(), g.clone(), vec![e1.clone(), e2.clone()]).await["transition"],
            "superseded"
        );
        assert_eq!(
            adopt(f.clone(), p.clone(), vec![e1.clone(), e2.clone()]).await["transition"],
            "superseded"
        );
        assert_eq!(active_of(&router, g_id).await, f["id"]);
        // Team A withdraws its own link; the chain below is team B's alone
        // again, B's regression counts, and P → F falls in the same request.
        let vetoed = compare(
            &router,
            &p,
            &bob,
            comparison_with(&p, &g, (3, 5), ("clean", "leaked"), family, "2"),
        )
        .await;
        assert_eq!(vetoed["transition"], "withdrawn", "{vetoed}");
        assert_eq!(active_of(&router, g_id).await, serde_json::json!(g_id));
        assert_eq!(
            get(&router, &format!("/v1/skills/{p_id}"), None)
                .await
                .get("superseded_by"),
            None
        );
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
        // H (team A) → Q (team B's fork), adopted by E1 and E2.
        let h = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Keep diagnostics on standard error so pipelines stay parseable.",
        )
        .await;
        let h_id = h["id"].as_str().unwrap();
        let q = fork_of(
            &router,
            &h,
            &m1,
            "Keep diagnostics on standard error so pipelines stay parseable and scripted.",
        )
        .await;
        verify_fork(&router, &q, [&m2, &m3]).await;
        assert_eq!(
            adopt(q.clone(), h.clone(), vec![e1.clone(), e2.clone()]).await["transition"],
            "superseded"
        );
        // A request authenticated before a revocation is judged on the
        // principal as it stands when the request runs.
        let stale = evaluators[0].0.clone();
        store
            .set_authorized_evaluator(&stale.id, false)
            .await
            .unwrap();
        assert_eq!(active_of(&router, h_id).await, serde_json::json!(h_id));
        let submission: s_code_skill_shop::ComparisonSubmission =
            serde_json::from_value(comparison(&q, &h, 3, 5, "clean", "2")).unwrap();
        let (recorded, _, _, transition) = store
            .record_comparison(&stale, q["id"].as_str().unwrap(), &submission)
            .await
            .unwrap();
        assert!(!recorded.authoritative);
        assert_eq!(transition, s_code_skill_shop::SupersessionTransition::None);
        store.disable_principal(&stale.id).await.unwrap();
        let submission: s_code_skill_shop::ComparisonSubmission =
            serde_json::from_value(comparison(&q, &h, 3, 5, "clean", "3")).unwrap();
        assert!(matches!(
            store
                .record_comparison(&stale, q["id"].as_str().unwrap(), &submission)
                .await,
            Err(StoreError::Forbidden)
        ));
        // A veto that also deprecates the parent withdraws the link in the
        // same request.
        assert_eq!(
            adopt(q.clone(), h.clone(), vec![e3.clone()]).await["transition"],
            "superseded"
        );
        let condemned = compare(
            &router,
            &q,
            &carol,
            comparison_with(&q, &h, (3, 5), ("leaked", "leaked"), family, "2"),
        )
        .await;
        assert_eq!(condemned["transition"], "withdrawn", "{condemned}");
        assert_eq!(condemned["parent"]["status"], "deprecated");
        assert_eq!(
            condemned["parent"].get("superseded_by"),
            None,
            "{condemned}"
        );
        assert_eq!(active_of(&router, h_id).await, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn installed_links_follow_the_authority_that_decided_them() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let mut evaluators = Vec::new();
        for name in ["E1", "E2", "E3", "E4"] {
            evaluators.push(
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap(),
            );
        }
        let [e1, e2, e3, e4] = [0, 1, 2, 3].map(|i| evaluators[i].1.clone());
        let s1 = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let s1_id = s1["id"].as_str().unwrap();
        let s2 = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &s2, [&m2, &m3]).await;
        let mut last = serde_json::Value::Null;
        for token in [&e1, &e2] {
            last = compare(
                &router,
                &s2,
                token,
                comparison(&s2, &s1, 3, 5, "clean", "1"),
            )
            .await;
        }
        assert_eq!(last["transition"], "superseded");
        // Revoking a counted evaluator releases the link it decided; a
        // grant never installs a link by itself.
        store
            .set_authorized_evaluator(&evaluators[1].0.id, false)
            .await
            .unwrap();
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        store
            .set_authorized_evaluator(&evaluators[1].0.id, true)
            .await
            .unwrap();
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        let readopted = compare(&router, &s2, &e1, comparison(&s2, &s1, 3, 5, "clean", "2")).await;
        assert_eq!(readopted["transition"], "superseded", "{readopted}");
        // Disabling one does the same.
        store.disable_principal(&evaluators[0].0.id).await.unwrap();
        assert_eq!(active_of(&router, s1_id).await, serde_json::json!(s1_id));
        let reasons: Vec<String> = store
            .events(Some("skill.supersession_released"))
            .await
            .unwrap()
            .iter()
            .map(|event| event["payload"]["reason"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(reasons, vec!["gate_no_longer_holds"; 2]);
        // One comparison that condemns a parent and its successor installs
        // nothing on the parent before deprecating it.
        let readopted = compare(&router, &s2, &e3, comparison(&s2, &s1, 3, 5, "clean", "1")).await;
        assert_eq!(readopted["transition"], "superseded", "{readopted}");
        let waiting = fork_of(
            &router,
            &s1,
            &m1,
            "Return a distinct exit status for malformed input and name the reason.",
        )
        .await;
        verify_fork(&router, &waiting, [&m2, &m3]).await;
        for token in [&e2, &e3] {
            let accepted = compare(
                &router,
                &waiting,
                token,
                comparison(&waiting, &s1, 3, 5, "clean", "1"),
            )
            .await;
            assert_eq!(accepted["transition"], "none", "the parent's link stands");
        }
        let condemned = compare(
            &router,
            &s2,
            &e4,
            comparison_with(
                &s2,
                &s1,
                (3, 5),
                ("leaked", "leaked"),
                Some("cli-error-contract"),
                "1",
            ),
        )
        .await;
        assert_eq!(condemned["fork"]["status"], "deprecated", "{condemned}");
        assert_eq!(condemned["parent"]["status"], "deprecated", "{condemned}");
        assert_eq!(
            condemned["parent"].get("superseded_by"),
            None,
            "{condemned}"
        );
    }

    #[tokio::test]
    async fn lineage_depth_and_per_team_quotas_bound_the_tree_and_it_renders() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let mut counter = 0;
        let mut next = || {
            counter += 1;
            format!("Return a distinct exit status for malformed input, variant {counter}.")
        };
        // Depth: a chain as deep as allowed, then one more.
        let (status, deep_root) = post(
            &router,
            "/v1/skills",
            &alice,
            publication(
                "Return a distinct exit status for malformed input.",
                SkillVisibility::Team,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let mut chain = vec![deep_root];
        for _ in 0..32 {
            let fork = fork_of(&router, chain.last().unwrap(), &alice, &next()).await;
            chain.push(fork);
        }
        let (status, refused) = post(
            &router,
            &format!("/v1/skills/{}/forks", chain[32]["id"].as_str().unwrap()),
            &alice,
            fork_body(&next(), APPLICABILITY, None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert!(refused.to_string().contains("deep"), "{refused}");

        // Quotas: a public root, forked by other teams.
        let root = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Keep diagnostics on standard error so pipelines stay parseable.",
        )
        .await;
        let root_id = root["id"].as_str().unwrap().to_owned();
        let mut teams = Vec::new();
        for n in 0..7 {
            let (_, token) = store
                .create_principal_with(&format!("Outsider{n}"), "org", &format!("team-o{n}"), true)
                .await
                .unwrap();
            teams.push(token);
        }
        // One team forks one skill at most four times.
        let mut entries = Vec::new();
        for _ in 0..4 {
            entries.push(fork_of(&router, &root, &teams[0], &next()).await);
        }
        let (status, refused) = post(
            &router,
            &format!("/v1/skills/{root_id}/forks"),
            &teams[0],
            fork_body(&next(), APPLICABILITY, None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        // Other teams share sixteen forks of the root; its own team keeps room.
        for token in &teams[1..4] {
            for _ in 0..4 {
                fork_of(&router, &root, token, &next()).await;
            }
        }
        let (status, refused) = post(
            &router,
            &format!("/v1/skills/{root_id}/forks"),
            &teams[4],
            fork_body(&next(), APPLICABILITY, None),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        let owners_fork = fork_of(&router, &root, &alice, &next()).await;
        // Each team fills its lineage quota of 32 under its own forks (four
        // per parent); later teams enter below the first team's fork. Other
        // teams share 223 forks of the tree; the 224th is refused.
        let mut other_forks = 16;
        let mut shared_refusal = None;
        for (index, token) in teams.iter().enumerate() {
            let me = get(&router, "/v1/me", Some(token)).await["id"].clone();
            let mut parents = std::collections::VecDeque::new();
            let mut made = 0;
            if index < 4 {
                let listed =
                    get(&router, &format!("/v1/skills/{root_id}/forks"), Some(token)).await;
                for fork in listed.as_array().unwrap() {
                    if fork["forked_by"]["id"] == me {
                        parents.push_back(fork.clone());
                        made += 1;
                    }
                }
            } else {
                parents.push_back(entries[0].clone());
            }
            while made < 32 && shared_refusal.is_none() {
                let parent = parents.pop_front().expect("a parent with room");
                for _ in 0..4 {
                    if made == 32 {
                        break;
                    }
                    let (status, fork) = post(
                        &router,
                        &format!("/v1/skills/{}/forks", parent["id"].as_str().unwrap()),
                        token,
                        fork_body(&next(), APPLICABILITY, None),
                    )
                    .await;
                    if status == StatusCode::CONFLICT {
                        shared_refusal = Some((other_forks, fork));
                        break;
                    }
                    assert_eq!(status, StatusCode::CREATED, "{fork}");
                    made += 1;
                    other_forks += 1;
                    parents.push_back(fork);
                }
            }
            if shared_refusal.is_some() {
                assert_eq!(index, 6);
                break;
            }
            let (status, refused) = post(
                &router,
                &format!(
                    "/v1/skills/{}/forks",
                    parents.front().unwrap()["id"].as_str().unwrap()
                ),
                token,
                fork_body(&next(), APPLICABILITY, None),
            )
            .await;
            assert_eq!(status, StatusCode::CONFLICT, "{refused}");
            assert!(
                refused.to_string().contains("32 forks in one lineage"),
                "{refused}"
            );
        }
        let (count, refused) = shared_refusal.expect("the shared room runs out");
        assert_eq!(count, 223, "{refused}");
        assert!(
            refused.to_string().contains("teams other than its root"),
            "{refused}"
        );
        // The root's team still has its own room.
        fork_of(&router, &owners_fork, &alice, &next()).await;
        let lineage = get(
            &router,
            &format!("/v1/skills/{root_id}/lineage"),
            Some(&teams[0]),
        )
        .await;
        assert_eq!(lineage["root_id"], serde_json::json!(root_id));
        assert_eq!(lineage["nodes"].as_array().unwrap().len(), 1 + 223 + 2);
        let (status, _, page) = send(
            &router,
            request(
                "GET",
                &format!("/shop/skills/{root_id}"),
                Some(&alice),
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.len() < 512 * 1024,
            "the page stays bounded: {} bytes",
            page.len()
        );
    }

    #[tokio::test]
    async fn challenges_are_capped_and_listed_in_pages() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let skill = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let skill_id = skill["id"].as_str().unwrap();
        let uri = format!("/v1/skills/{skill_id}/challenges");
        let claim = |n: usize| format!("The rule fails for library callers, case {n}.");
        // The oldest challenge is the only evidence-backed one.
        let receipts = get(&router, &format!("/v1/skills/{skill_id}/evaluations"), None).await;
        let receipt_id = receipts[0]["id"].as_str().unwrap().to_owned();
        let (_, witness) = principal(&store, "Witness", "team-w").await;
        let (status, backed) = post(
            &router,
            &uri,
            &witness,
            challenge(
                "negative_transfer",
                "The rule regresses library callers.",
                Some(&receipt_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{backed}");
        let mut filed = 1;
        for who in 0..20 {
            let (_, token) =
                principal(&store, &format!("Challenger{who}"), &format!("team-c{who}")).await;
            for n in 0..10 {
                if filed == 200 {
                    break;
                }
                let (status, body) = post(
                    &router,
                    &uri,
                    &token,
                    challenge("applicability_failure", &claim(who * 10 + n), None),
                )
                .await;
                assert_eq!(status, StatusCode::CREATED, "{body}");
                filed += 1;
            }
            if who == 0 {
                let (status, _) = post(
                    &router,
                    &uri,
                    &token,
                    challenge("applicability_failure", &claim(999), None),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::CONFLICT,
                    "one principal files at most 10 challenges per skill"
                );
            }
        }
        assert_eq!(filed, 200);
        let (_, late) = principal(&store, "Late", "team-late").await;
        let (status, _) = post(
            &router,
            &uri,
            &late,
            challenge("applicability_failure", &claim(1000), None),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "a skill collects at most 200 challenges"
        );
        for (query, expected) in [("", 50), ("?limit=5", 5), ("?limit=1000", 100)] {
            assert_eq!(
                get(&router, &format!("{uri}{query}"), None)
                    .await
                    .as_array()
                    .unwrap()
                    .len(),
                expected
            );
        }
        for query in [
            "?limit=5&extra=1",
            "?show=everything",
            "?before=challenge_unknown",
        ] {
            let (status, _, _) = send(
                &router,
                request("GET", &format!("{uri}{query}"), None, None),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        }
        // The filter applies before the page is cut, and the cursor reads
        // every challenge exactly once, newest first.
        let backed_only = get(&router, &format!("{uri}?show=evidence&limit=10"), None).await;
        assert_eq!(backed_only.as_array().unwrap().len(), 1);
        assert_eq!(backed_only[0]["id"], backed["id"]);
        let mut seen = Vec::new();
        let mut before: Option<String> = None;
        loop {
            let query = match &before {
                Some(id) => format!("{uri}?limit=100&before={id}"),
                None => format!("{uri}?limit=100"),
            };
            let page = get(&router, &query, None).await;
            let page = page.as_array().unwrap();
            if page.is_empty() {
                break;
            }
            for item in page {
                seen.push(item["id"].as_str().unwrap().to_owned());
            }
            before = seen.last().cloned();
        }
        assert_eq!(seen.len(), 200);
        assert_eq!(
            seen.iter().collect::<std::collections::BTreeSet<_>>().len(),
            200
        );
        assert_eq!(
            seen.last(),
            backed["id"].as_str().map(str::to_owned).as_ref()
        );
        // The web page filters the same way and links to older entries.
        let (_, _, filtered) = send(
            &router,
            request(
                "GET",
                &format!("/shop/skills/{skill_id}?challenges=evidence"),
                None,
                None,
            ),
        )
        .await;
        assert!(filtered.contains("The rule regresses library callers."));
        let (_, _, first) = send(
            &router,
            request("GET", &format!("/shop/skills/{skill_id}"), None, None),
        )
        .await;
        assert!(
            first.contains("challenges_before="),
            "a full page links onward"
        );
    }

    #[tokio::test]
    async fn links_to_skills_the_viewer_cannot_see_are_hidden() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, e1) = store
            .create_principal_with("E1", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e2) = store
            .create_principal_with("E2", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, e3) = store
            .create_principal_with("E3", "org", "eval-team", true)
            .await
            .unwrap();
        let (_, outsider) = principal(&store, "Outsider", "team-y").await;
        let (_, candidate) = post(
            &router,
            "/v1/skills",
            &alice,
            publication(
                "Return a distinct exit status for malformed input.",
                SkillVisibility::Public,
            ),
        )
        .await;
        let candidate_id = candidate["id"].as_str().unwrap();
        // An authorized evaluator challenges the public candidate and forks
        // it in answer; its own team verifies the fork.
        let (status, filed) = post(
            &router,
            &format!("/v1/skills/{candidate_id}/challenges"),
            &e1,
            challenge(
                "applicability_failure",
                "The rule fails for library callers.",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{filed}");
        let challenge_id = filed["id"].as_str().unwrap().to_owned();
        let (status, fork) = post(
            &router,
            &format!("/v1/skills/{candidate_id}/forks"),
            &e1,
            fork_body(
                "Return a distinct exit status for malformed input and name the field.",
                APPLICABILITY,
                Some(&challenge_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{fork}");
        let fork_id = fork["id"].as_str().unwrap();
        verify_fork(&router, &fork, [&e2, &e3]).await;
        for token in [Some(outsider.as_str()), None] {
            let seen = get(&router, &format!("/v1/skills/{fork_id}"), token).await;
            assert_eq!(seen["status"], "verified");
            assert!(
                seen.get("parent_skill_id").is_none_or(|p| p.is_null()),
                "{seen}"
            );
            assert!(
                seen.get("responding_to_challenge_id")
                    .is_none_or(|c| c.is_null()),
                "the answered challenge of a hidden parent stays hidden: {seen}"
            );
            let lineage = get(&router, &format!("/v1/skills/{fork_id}/lineage"), token).await;
            assert_eq!(lineage["root_id"], serde_json::json!(fork_id));
            assert!(!lineage.to_string().contains(candidate_id), "{lineage}");
            assert!(!lineage.to_string().contains(&challenge_id), "{lineage}");
            let (_, _, page) = send(
                &router,
                request("GET", &format!("/shop/skills/{fork_id}"), token, None),
            )
            .await;
            assert!(!page.contains(candidate_id));
        }
        assert_eq!(
            get(&router, &format!("/v1/skills/{fork_id}"), Some(&e1)).await["parent_skill_id"],
            serde_json::json!(candidate_id)
        );
    }

    #[tokio::test]
    async fn every_write_uses_current_standing_and_vetoes_outlive_their_author() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (bob_principal, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (dave, _) = principal(&store, "Dave", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let mut evaluators = Vec::new();
        for name in ["E1", "E2", "E3"] {
            evaluators.push(
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap(),
            );
        }
        let [e1, e2, e3] = [0, 1, 2].map(|i| evaluators[i].1.clone());
        let family = Some("cli-error-contract");
        let p = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let p_id = p["id"].as_str().unwrap();
        // A principal disabled after its request was authenticated writes
        // nothing: no deprecation, fork, challenge or publication.
        store.disable_principal(&dave.id).await.unwrap();
        let fork: s_code_skill_shop::ForkSubmission = serde_json::from_value(fork_body(
            "Return a distinct exit status for malformed input and log it.",
            APPLICABILITY,
            None,
        ))
        .unwrap();
        let claim: s_code_skill_shop::ChallengeSubmission = serde_json::from_value(challenge(
            "correctness_failure",
            "The rule misses library callers.",
            None,
        ))
        .unwrap();
        let fresh: SkillPublication = serde_json::from_value(publication(
            "Keep diagnostics on standard error.",
            SkillVisibility::Public,
        ))
        .unwrap();
        assert!(matches!(
            store.deprecate(&dave, p_id, "stale").await,
            Err(StoreError::Forbidden)
        ));
        assert!(matches!(
            store.create_fork(&dave, p_id, &fork).await,
            Err(StoreError::Forbidden)
        ));
        assert!(matches!(
            store.create_challenge(&dave, p_id, &claim).await,
            Err(StoreError::Forbidden)
        ));
        assert!(matches!(
            store.publish(&dave, &fresh).await,
            Err(StoreError::Forbidden)
        ));
        assert_eq!(
            get(&router, &format!("/v1/skills/{p_id}"), None).await["status"],
            "verified"
        );
        // A parent-team safety veto blocks the fork for good, also after its
        // author is disabled.
        let f = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &f, [&m2, &m3]).await;
        for token in [&e1, &e2] {
            compare(&router, &f, token, comparison(&f, &p, 3, 5, "clean", "1")).await;
        }
        assert_eq!(active_of(&router, p_id).await, f["id"]);
        let vetoed = compare(
            &router,
            &f,
            &bob,
            comparison_with(&f, &p, (3, 5), ("clean", "leaked"), family, "1"),
        )
        .await;
        assert_eq!(vetoed["transition"], "withdrawn", "{vetoed}");
        store.disable_principal(&bob_principal.id).await.unwrap();
        for token in [&e1, &e2, &e3] {
            let again = compare(&router, &f, token, comparison(&f, &p, 3, 5, "clean", "2")).await;
            assert_eq!(again["transition"], "none", "{again}");
        }
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
        // A fork-arm veto that deprecates the successor reports the
        // withdrawal of its link.
        let g = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the reason.",
        )
        .await;
        verify_fork(&router, &g, [&m2, &m3]).await;
        let mut last = serde_json::Value::Null;
        for token in [&e1, &e2] {
            last = compare(&router, &g, token, comparison(&g, &p, 3, 5, "clean", "1")).await;
        }
        assert_eq!(last["transition"], "superseded", "{last}");
        let condemned = compare(
            &router,
            &g,
            &e3,
            comparison_with(&g, &p, (3, 5), ("clean", "leaked"), family, "1"),
        )
        .await;
        assert_eq!(condemned["fork"]["status"], "deprecated", "{condemned}");
        assert_eq!(condemned["transition"], "withdrawn", "{condemned}");
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
    }

    #[tokio::test]
    async fn unreadable_rows_fail_closed_and_never_take_pages_down() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (bob_principal, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (_, dave) = principal(&store, "Dave", "team-a").await;
        let (_, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let mut evaluators = Vec::new();
        for name in ["E1", "E2", "E3"] {
            evaluators.push(
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap(),
            );
        }
        let [e1, e2, e3] = [0, 1, 2].map(|i| evaluators[i].1.clone());
        let [e1_id, e2_id] = [0, 1].map(|i| evaluators[i].0.id.clone());
        let family = Some("cli-error-contract");
        let released = |store: Arc<RegistryStore>| async move {
            store
                .events(Some("skill.supersession_released"))
                .await
                .unwrap()
                .iter()
                .map(|event| event["payload"]["reason"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        let p = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let p_id = p["id"].as_str().unwrap();
        // An unreadable veto blocks the fork instead of vanishing.
        let q = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the field.",
        )
        .await;
        verify_fork(&router, &q, [&m2, &m3]).await;
        let q_id = q["id"].as_str().unwrap();
        let vetoed = compare(
            &router,
            &q,
            &bob,
            comparison_with(&q, &p, (3, 5), ("clean", "leaked"), family, "1"),
        )
        .await;
        assert_eq!(vetoed["transition"], "none", "{vetoed}");
        store
            .write_behind_the_back(&[format!(
                "UPDATE comparisons SET created_at='not a time' WHERE fork_id='{q_id}' AND principal_id='{}'",
                bob_principal.id
            )])
            .await;
        for token in [&e1, &e2] {
            let clean = compare(&router, &q, token, comparison(&q, &p, 3, 5, "clean", "1")).await;
            assert_eq!(clean["transition"], "none", "{clean}");
        }
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
        // An installed link whose evidence became unreadable is released at
        // its next check.
        let r = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the reason.",
        )
        .await;
        verify_fork(&router, &r, [&m2, &m3]).await;
        let r_id = r["id"].as_str().unwrap();
        let mut last = serde_json::Value::Null;
        for token in [&e1, &e2] {
            last = compare(&router, &r, token, comparison(&r, &p, 3, 5, "clean", "1")).await;
        }
        assert_eq!(last["transition"], "superseded", "{last}");
        store
            .write_behind_the_back(&[format!(
                "UPDATE comparisons SET fork_safety='bogus' WHERE fork_id='{r_id}' AND principal_id='{e1_id}'"
            )])
            .await;
        let checked = compare(&router, &r, &e3, comparison(&r, &p, 3, 5, "clean", "1")).await;
        assert_eq!(checked["transition"], "withdrawn", "{checked}");
        assert!(
            released(store.clone())
                .await
                .contains(&"unreadable".to_owned())
        );
        // A link no query can name (a non-text successor) is released by the
        // walk below a released link and by the startup pass, and neither
        // fails.
        let t = fork_of(
            &router,
            &p,
            &m1,
            "Return a distinct exit status for malformed input and name the flag.",
        )
        .await;
        verify_fork(&router, &t, [&m2, &m3]).await;
        let t_id = t["id"].as_str().unwrap();
        for token in [&e1, &e2] {
            last = compare(&router, &t, token, comparison(&t, &p, 3, 5, "clean", "1")).await;
        }
        assert_eq!(last["transition"], "superseded", "{last}");
        store
            .write_behind_the_back(&[format!(
                "UPDATE skills SET superseded_by=X'00ff' WHERE id='{t_id}'"
            )])
            .await;
        store.disable_principal(&e2_id).await.unwrap();
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
        assert!(
            get(&router, &format!("/v1/skills/{t_id}"), None)
                .await
                .get("superseded_by")
                .is_none_or(serde_json::Value::is_null)
        );
        store
            .write_behind_the_back(&[format!(
                "UPDATE skills SET superseded_by=X'00ff' WHERE id='{q_id}'"
            )])
            .await;
        assert_eq!(store.revalidate_supersessions().await.unwrap(), 1);
        assert_eq!(
            get(&router, &format!("/v1/skills/{q_id}"), None).await["status"],
            "verified"
        );
        // An unreadable receipt blocks verification.
        let (_, s) = post(
            &router,
            "/v1/skills",
            &alice,
            publication(
                "Keep diagnostics on standard error so pipelines stay parseable.",
                SkillVisibility::Public,
            ),
        )
        .await;
        let s_id = s["id"].as_str().unwrap();
        let (status, _) = post(
            &router,
            &format!("/v1/skills/{s_id}/evaluations"),
            &bob,
            receipt(&s, 3, 4, "clean", "1"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        store
            .write_behind_the_back(&[format!(
                "UPDATE receipts SET safety='bogus' WHERE skill_id='{s_id}'"
            )])
            .await;
        for token in [&carol, &dave] {
            let (status, body) = post(
                &router,
                &format!("/v1/skills/{s_id}/evaluations"),
                token,
                receipt(&s, 3, 4, "clean", "1"),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            assert_eq!(body["skill"]["status"], "candidate", "{body}");
        }
        // Unreadable linked rows are hidden rather than failing the pages
        // that link to them.
        store
            .write_behind_the_back(&[format!(
                "UPDATE skills SET created_at='not a time' WHERE id='{p_id}'"
            )])
            .await;
        for uri in [
            "/v1/skills".to_owned(),
            "/v1/skills?status=verified".to_owned(),
            format!("/v1/skills/{q_id}"),
            format!("/v1/skills/{q_id}/comparisons"),
            "/shop".to_owned(),
            format!("/shop/skills/{q_id}"),
        ] {
            let (status, _, body) = send(&router, request("GET", &uri, None, None)).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
        let seen = get(&router, &format!("/v1/skills/{q_id}"), None).await;
        assert!(
            seen.get("parent_skill_id")
                .is_none_or(serde_json::Value::is_null),
            "{seen}"
        );
    }

    /// A link whose successor column no query can name is released like any
    /// other: the challenge its successor had addressed is open again and
    /// the parent settles anew.
    #[tokio::test]
    async fn an_unnameable_link_reopens_its_challenge_and_settles_the_parent() {
        let (store, router) = registry().await;
        let (_, alice) = principal(&store, "Alice", "team-a").await;
        let (_, bob) = principal(&store, "Bob", "team-a").await;
        let (_, carol) = principal(&store, "Carol", "team-a").await;
        let (mallory, m1) = principal(&store, "Mallory1", "team-b").await;
        let (_, m2) = principal(&store, "Mallory2", "team-b").await;
        let (_, m3) = principal(&store, "Mallory3", "team-b").await;
        let mut evaluators = Vec::new();
        for name in ["E1", "E2"] {
            evaluators.push(
                store
                    .create_principal_with(name, "org", "eval-team", true)
                    .await
                    .unwrap(),
            );
        }
        let [e1, e2] = [0, 1].map(|i| evaluators[i].1.clone());
        let p = verified_public(
            &router,
            &alice,
            [&bob, &carol],
            "Return a distinct exit status for malformed input.",
        )
        .await;
        let p_id = p["id"].as_str().unwrap();
        let (status, filed) = post(
            &router,
            &format!("/v1/skills/{p_id}/challenges"),
            &e1,
            challenge(
                "correctness_failure",
                "The rule misses library callers.",
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{filed}");
        let challenge_id = filed["id"].as_str().unwrap().to_owned();
        let (status, fork) = post(
            &router,
            &format!("/v1/skills/{p_id}/forks"),
            &m1,
            fork_body(
                "Return a distinct exit status for malformed input and name the field.",
                APPLICABILITY,
                Some(&challenge_id),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{fork}");
        verify_fork(&router, &fork, [&m2, &m3]).await;
        let mut last = serde_json::Value::Null;
        for token in [&e1, &e2] {
            last = compare(
                &router,
                &fork,
                token,
                comparison(&fork, &p, 3, 5, "clean", "1"),
            )
            .await;
        }
        assert_eq!(last["transition"], "superseded", "{last}");
        let addressed = get(&router, &format!("/v1/skills/{p_id}/challenges"), None).await;
        assert_eq!(addressed[0]["status"], "addressed", "{addressed}");
        // The stored link becomes a value no query can name, and the only
        // other candidate is deprecated, so nothing can take its place.
        store
            .write_behind_the_back(&[format!(
                "UPDATE skills SET superseded_by=X'00ff' WHERE id='{p_id}'"
            )])
            .await;
        store
            .deprecate(&mallory, fork["id"].as_str().unwrap(), "no longer correct")
            .await
            .unwrap();
        assert_eq!(store.revalidate_supersessions().await.unwrap(), 1);
        let reopened = get(&router, &format!("/v1/skills/{p_id}/challenges"), None).await;
        assert_eq!(reopened[0]["status"], "open", "{reopened}");
        assert_eq!(active_of(&router, p_id).await, serde_json::json!(p_id));
    }
}
