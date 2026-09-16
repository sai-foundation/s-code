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
        if self.insecure_cookies && !self.bind.ip().is_loopback() {
            anyhow::bail!(
                "S_CODE_SKILL_REGISTRY_INSECURE_COOKIES is a loopback development setting; it is refused for bind {}",
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
        SkillArtifact, SkillProvenance, SkillPublication, SkillReceiptSubmission, SkillStatus,
        SkillVisibility, skill_content_digest,
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
            "content_digest": skill_content_digest(lesson, APPLICABILITY, 1),
            "sanitization_version": 1,
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
            1
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
        // Loopback development cookies are refused off loopback, even behind a proxy.
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
                insecure_cookies: true,
                ..config.clone()
            }
            .validate()
            .is_ok()
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
            1
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
}
