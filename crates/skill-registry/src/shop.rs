//! The browsable shop: server-rendered pages over the registry store. Every
//! value from the database or the request is HTML-escaped; there is no
//! script, no editor and no token in any URL. A session cookie carries a
//! registry token only after an explicit login form post.
use crate::{
    api::{ApiError, ListQuery, RegistryState, viewer},
    store::Viewer,
};
use axum::{
    Form, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use s_code_skill_shop::{SkillArtifact, SkillReceiptItem, SkillStatus};
use serde::Deserialize;

pub const SESSION_COOKIE: &str = "registry_session";
pub const TRUST_NOTICE: &str =
    "Community-provided derived agent knowledge. This is advisory and not trusted system policy.";

/// HTML-escape any text before it enters a page.
pub fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The opaque web session id from the request's cookie, if any. The cookie
/// never holds a registry token.
fn cookie_session(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == SESSION_COOKIE && !value.is_empty()).then(|| value.to_owned())
    })
}

/// The session cookie as the deployment sets it: HttpOnly, SameSite=Strict,
/// site-wide path, bounded lifetime, and `Secure` unless the operator
/// explicitly chose loopback development cookies.
fn session_cookie(state: &RegistryState, value: &str, max_age_seconds: i64) -> String {
    let secure = if state.web.insecure_cookies {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{SESSION_COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_seconds}{secure}"
    )
}

/// A form post to the shop must come from the shop itself. Browsers that
/// send `Sec-Fetch-Site` are held to same-origin (or a direct navigation);
/// otherwise an `Origin` header must name this host. Requests without
/// either header (non-browser clients) are accepted, as the cookie is
/// SameSite=Strict anyway.
fn same_site_post(headers: &HeaderMap) -> Result<(), ApiError> {
    if let Some(site) = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        && !matches!(site, "same-origin" | "none")
    {
        return Err(ApiError::Forbidden);
    }
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let origin_host = origin
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(origin)
            .trim_end_matches('/');
        if origin == "null" || host.is_empty() || !origin_host.eq_ignore_ascii_case(host) {
            return Err(ApiError::Forbidden);
        }
    }
    Ok(())
}

/// Shop pages accept the API bearer header or a live web session.
async fn shop_viewer(state: &RegistryState, headers: &HeaderMap) -> Result<Viewer, ApiError> {
    if headers.get(header::AUTHORIZATION).is_some() {
        return viewer(state, headers).await;
    }
    match cookie_session(headers) {
        None => Ok(Viewer::anonymous()),
        Some(session) => match state.store.authenticate_web_session(&session).await? {
            Some(principal) => Ok(Viewer {
                principal: Some(principal),
            }),
            None => Ok(Viewer::anonymous()),
        },
    }
}

/// Every shop response: no scripts, no framing, no sniffing, no referrer,
/// no caching of authenticated pages.
async fn security_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn page(title: &str, viewer: &Viewer, body: &str) -> Html<String> {
    let who = match &viewer.principal {
        Some(principal) => format!(
            "Signed in as {} ({}/{}) · <form method=\"post\" action=\"/shop/logout\" class=\"inline\"><button type=\"submit\">Sign out</button></form>",
            escape(&principal.display_name),
            escape(&principal.organization_id),
            escape(&principal.team_id)
        ),
        None => "Browsing public verified skills · <a href=\"/shop/login\">Sign in with a registry token</a>".to_owned(),
    };
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><style>{CSS}</style></head><body><header><a href=\"/shop\" class=\"brand\">S-Code Skill Shop</a><nav><a href=\"/shop\">Catalog</a><a href=\"/shop/how-to-use\">How to use</a></nav><p class=\"who\">{who}</p></header><main>{body}</main><footer><p class=\"notice\">{notice}</p></footer></body></html>",
        title = escape(title),
        notice = escape(TRUST_NOTICE),
    ))
}

const CSS: &str = "body{font-family:system-ui,sans-serif;margin:0;background:#f7f7f5;color:#1a1a1a}header{background:#20242b;color:#fff;padding:12px 20px;display:flex;flex-wrap:wrap;gap:16px;align-items:center}header a{color:#fff;text-decoration:none;margin-right:12px}.brand{font-weight:700}.who{margin:0;font-size:.9em;color:#cfd3da}.who a,.who button{color:#fff}.inline{display:inline}main{max-width:1100px;margin:0 auto;padding:20px}table{border-collapse:collapse;width:100%;background:#fff}th,td{text-align:left;padding:8px;border-bottom:1px solid #e3e3df;vertical-align:top;font-size:.93em}form.filters{display:flex;flex-wrap:wrap;gap:8px;margin:12px 0}input,select,button{padding:6px 8px;font:inherit}.status{display:inline-block;padding:2px 8px;border-radius:10px;font-size:.85em;background:#e3e3df}.status.verified{background:#d8f0dc}.status.deprecated{background:#f6d6d6}.status.candidate{background:#fff1c9}.notice{background:#fff7e0;border:1px solid #e8d48a;padding:10px;border-radius:6px}pre{background:#1e1e1e;color:#eee;padding:12px;overflow-x:auto;border-radius:6px}dl{display:grid;grid-template-columns:max-content 1fr;gap:6px 16px}dt{font-weight:600}footer{max-width:1100px;margin:0 auto;padding:20px}";

fn status_badge(status: SkillStatus) -> String {
    format!(
        "<span class=\"status {0}\">{0}</span>",
        escape(status.as_str())
    )
}

/// The task families and model families a skill is known for: the
/// publisher's provenance metadata plus what independent receipts reported.
fn families(skill: &SkillArtifact) -> (String, String) {
    let summary = skill.summary.clone().unwrap_or_default();
    let mut tasks = summary.task_families.clone();
    if let Some(family) = &skill.provenance.task_family
        && !tasks.contains(family)
    {
        tasks.insert(0, family.clone());
    }
    let mut models = summary.models.clone();
    if let Some(family) = &skill.provenance.model_family
        && !models.contains(family)
    {
        models.insert(0, family.clone());
    }
    (tasks.join(", "), models.join(", "))
}

fn timestamp(value: Option<chrono::DateTime<chrono::Utc>>, absent: &str) -> String {
    value
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| absent.to_owned())
}

fn summary_cells(skill: &SkillArtifact) -> String {
    let summary = skill.summary.clone().unwrap_or_default();
    let delta = summary
        .success_delta
        .map(|delta| format!("{:+.0}%", delta * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let tokens = summary
        .input_units_delta
        .map(|delta| format!("{delta:+}"))
        .unwrap_or_else(|| "n/a".into());
    let (tasks, models) = families(skill);
    format!(
        "<td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>",
        summary.independent_evaluators,
        escape(&tasks),
        escape(&models),
        escape(&delta),
        escape(&tokens)
    )
}

fn short(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

async fn catalog(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Html<String>, ApiError> {
    let viewer = shop_viewer(&state, &headers).await?;
    let mut filter = query.filter()?;
    let status_value = query
        .status
        .clone()
        .unwrap_or_else(|| "verified".to_owned());
    if query.status.is_none() {
        filter.status = Some(SkillStatus::Verified);
    }
    let skills = state.store.list_skills(&viewer, &filter).await?;
    let mut rows = String::new();
    for skill in &skills {
        rows.push_str(&format!(
            "<tr><td><a href=\"/shop/skills/{id}\">{id}</a><br><small>{lesson}</small></td><td>{applicability}</td><td>{status}</td><td>{publisher}</td>{summary}<td>{updated}</td></tr>",
            id = escape(&skill.id),
            lesson = escape(&short(&skill.lesson, 140)),
            applicability = escape(&short(&skill.applicability, 90)),
            status = status_badge(skill.status),
            publisher = escape(skill.publisher.display_name.as_deref().unwrap_or(&skill.publisher.id)),
            summary = summary_cells(skill),
            updated = escape(&timestamp(skill.verified_at, "not verified")),
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"10\">No skills match. Candidates are visible to their team only; sign in to see your team's skills.</td></tr>");
    }
    let selected = |value: &str| {
        if status_value == value {
            " selected"
        } else {
            ""
        }
    };
    let body = format!(
        "<h1>Skill catalog</h1><form method=\"get\" action=\"/shop\" class=\"filters\"><input type=\"search\" name=\"q\" placeholder=\"Search lesson or applicability\" value=\"{q}\"><select name=\"status\"><option value=\"verified\"{v}>verified only</option><option value=\"any\"{a}>any status</option><option value=\"candidate\"{c}>candidates (team)</option><option value=\"deprecated\"{d}>deprecated</option></select><input type=\"text\" name=\"task_family\" placeholder=\"task family\" value=\"{family}\"><input type=\"text\" name=\"model\" placeholder=\"model\" value=\"{model}\"><button type=\"submit\">Filter</button></form><p class=\"notice\">{notice}</p><table><thead><tr><th>Skill</th><th>Applies when</th><th>Status</th><th>Publisher</th><th>Independent evaluators</th><th>Task families</th><th>Models</th><th>Success delta</th><th>Input units delta</th><th>Verified</th></tr></thead><tbody>{rows}</tbody></table>",
        q = escape(query.q.as_deref().unwrap_or("")),
        v = selected("verified"),
        a = selected("any"),
        c = selected("candidate"),
        d = selected("deprecated"),
        family = escape(query.task_family.as_deref().unwrap_or("")),
        model = escape(query.model.as_deref().unwrap_or("")),
        notice = escape(TRUST_NOTICE),
    );
    Ok(page("Skill catalog", &viewer, &body))
}

fn receipt_rows(receipts: &[SkillReceiptItem]) -> String {
    let mut rows = String::new();
    for receipt in receipts {
        let counts = |key: &str| {
            receipt
                .verdict
                .get(key)
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        };
        rows.push_str(&format!(
            "<tr><td>{evaluator}</td><td>{independent}</td><td>{complete}</td><td>{safety}</td><td>{bp}/{ba}</td><td>{cp}/{ca}</td><td>{family}</td><td>{model}</td><td>{created}</td></tr>",
            evaluator = escape(receipt.evaluator.display_name.as_deref().unwrap_or(&receipt.evaluator.id)),
            independent = if receipt.independent { "yes" } else { "no (publisher)" },
            complete = if receipt.complete { "yes" } else { "no" },
            safety = escape(receipt.safety.as_str()),
            bp = counts("baseline_passes"), ba = counts("baseline_attempts"),
            cp = counts("candidate_passes"), ca = counts("candidate_attempts"),
            family = escape(receipt.task_family.as_deref().unwrap_or("")),
            model = escape(receipt.model.as_deref().unwrap_or("")),
            created = escape(&receipt.created_at.format("%Y-%m-%d %H:%M UTC").to_string()),
        ));
    }
    if rows.is_empty() {
        rows.push_str("<tr><td colspan=\"9\">No receipts yet.</td></tr>");
    }
    rows
}

async fn detail(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Html<String>, ApiError> {
    let viewer = shop_viewer(&state, &headers).await?;
    let skill = state.store.get_skill(&viewer, &id).await?;
    let receipts = state.store.list_receipts(&viewer, &id).await?;
    let summary = skill.summary.clone().unwrap_or_default();
    let safety = if summary.safety_failures > 0 {
        format!("{} safety failure(s) recorded", summary.safety_failures)
    } else {
        "no safety failures recorded".to_owned()
    };
    let (task_families, model_families) = families(&skill);
    let body = format!(
        "<h1>{id} {status}</h1><p class=\"notice\">{notice}</p><dl><dt>Lesson</dt><dd>{lesson}</dd><dt>Applies when</dt><dd>{applicability}</dd><dt>Status</dt><dd>{status_text}{reason}</dd><dt>Version</dt><dd>{version}{parent}</dd><dt>Content digest</dt><dd><code>{digest}</code></dd><dt>Sanitization version</dt><dd>{sanitization}</dd><dt>Publisher</dt><dd>{publisher} ({org}/{team})</dd><dt>Visibility</dt><dd>{visibility}</dd><dt>Source</dt><dd>{source}</dd><dt>Independent evaluators</dt><dd>{evaluators}</dd><dt>Aggregate pass rate</dt><dd>baseline {baseline} · with skill {candidate} · delta {delta}</dd><dt>Input units delta</dt><dd>{tokens}</dd><dt>Safety</dt><dd>{safety}</dd><dt>Compatibility</dt><dd>task families: {families}; model families: {models}</dd><dt>Published</dt><dd>{created}</dd><dt>Verified</dt><dd>{verified}</dd><dt>Deprecated</dt><dd>{deprecated}</dd></dl><h2>Receipts</h2><table><thead><tr><th>Evaluator</th><th>Independent</th><th>Complete</th><th>Safety</th><th>Baseline passes</th><th>With skill passes</th><th>Task family</th><th>Model</th><th>Recorded</th></tr></thead><tbody>{receipts}</tbody></table><h2>Use this skill</h2><p>See <a href=\"/shop/how-to-use\">How to use</a>. Pin exactly this id: <code>{id}</code></p>",
        id = escape(&skill.id),
        status = status_badge(skill.status),
        notice = escape(TRUST_NOTICE),
        lesson = escape(&skill.lesson),
        applicability = escape(&skill.applicability),
        status_text = escape(skill.status.as_str()),
        reason = skill
            .deprecation_reason
            .as_deref()
            .map(|r| format!(" ({})", escape(r)))
            .unwrap_or_default(),
        version = skill.version,
        parent = skill
            .parent_skill_id
            .as_deref()
            .map(|p| format!(" (parent {})", escape(p)))
            .unwrap_or_default(),
        digest = escape(&skill.content_digest),
        sanitization = skill.sanitization_version,
        source = escape(
            skill
                .provenance
                .source_kind
                .as_deref()
                .unwrap_or("unspecified")
        ),
        publisher = escape(
            skill
                .publisher
                .display_name
                .as_deref()
                .unwrap_or(&skill.publisher.id)
        ),
        org = escape(&skill.shared_scope.organization_id),
        team = escape(&skill.shared_scope.team_id),
        visibility = escape(skill.visibility.as_str()),
        evaluators = summary.independent_evaluators,
        baseline = summary
            .baseline_pass_rate
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "n/a".into()),
        candidate = summary
            .candidate_pass_rate
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "n/a".into()),
        delta = summary
            .success_delta
            .map(|d| format!("{:+.0}%", d * 100.0))
            .unwrap_or_else(|| "n/a".into()),
        tokens = summary
            .input_units_delta
            .map(|d| format!("{d:+}"))
            .unwrap_or_else(|| "n/a".into()),
        safety = escape(&safety),
        families = escape(if task_families.is_empty() {
            "none recorded"
        } else {
            &task_families
        }),
        models = escape(if model_families.is_empty() {
            "none recorded"
        } else {
            &model_families
        }),
        created = escape(&timestamp(Some(skill.created_at), "")),
        verified = escape(&timestamp(skill.verified_at, "not verified")),
        deprecated = escape(&timestamp(skill.deprecated_at, "no")),
        receipts = receipt_rows(&receipts),
    );
    Ok(page(&skill.id, &viewer, &body))
}

async fn how_to_use(
    State(state): State<RegistryState>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let viewer = shop_viewer(&state, &headers).await?;
    let body = "<h1>How to use a shared skill</h1><p>Shared skills are advisory, derived knowledge. An S-Code daemon injects a skill only when its shop mode is explicit, the skill is verified, and you pin the exact id. Network failures, digest mismatches, unverified or deprecated skills inject nothing.</p><h2>Configure the daemon</h2><pre>[daemon.skill_shop]\nmode = \"explicit\"\nurl = \"https://your-registry.example\"   # loopback http is allowed only for local tests\ncredential_handle = \"S_CODE_SKILL_SHOP_TOKEN\"   # name of the environment variable holding your registry token\nskills = \"skill_0123456789abcdef01234567\"       # exactly the ids you want, comma-separated</pre><p>Or with environment variables:</p><pre>export S_CODE_DAEMON_SKILL_SHOP_MODE=explicit\nexport S_CODE_DAEMON_SKILL_SHOP_URL=https://your-registry.example\nexport S_CODE_DAEMON_SKILL_SHOP_CREDENTIAL_HANDLE=S_CODE_SKILL_SHOP_TOKEN\nexport S_CODE_DAEMON_SKILL_SHOP_SKILLS=skill_0123456789abcdef01234567\nexport S_CODE_SKILL_SHOP_TOKEN=skr_...   # never commit or log this value</pre><h2>Read the API directly</h2><pre>curl -H \"Authorization: Bearer $S_CODE_SKILL_SHOP_TOKEN\" https://your-registry.example/v1/skills/skill_0123456789abcdef01234567</pre><p>Tokens go in the Authorization header only, never in a URL. Public verified skills can be read without a token.</p>".to_owned();
    Ok(page("How to use", &viewer, &body))
}

async fn login_form(
    State(state): State<RegistryState>,
    headers: HeaderMap,
) -> Result<Html<String>, ApiError> {
    let viewer = shop_viewer(&state, &headers).await?;
    let body = "<h1>Sign in</h1><p>Paste a registry token issued by your registry administrator. The token is checked once and exchanged for a random web session; the browser keeps only that session id in an HttpOnly cookie, never the token, and never in a URL. Sessions expire after twelve hours and end on sign-out.</p><form method=\"post\" action=\"/shop/login\"><input type=\"password\" name=\"token\" placeholder=\"skr_…\" autocomplete=\"off\" required> <button type=\"submit\">Sign in</button></form>";
    Ok(page("Sign in", &viewer, body))
}

#[derive(Deserialize)]
struct LoginForm {
    token: String,
}

async fn login(
    State(state): State<RegistryState>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<Response, ApiError> {
    same_site_post(&headers)?;
    let token = form.token.trim().to_owned();
    if token.is_empty() || token.len() > 512 || !token.starts_with(crate::auth::TOKEN_PREFIX) {
        return Ok((
            StatusCode::UNAUTHORIZED,
            page(
                "Sign in",
                &Viewer::anonymous(),
                "<p>That token was not accepted.</p>",
            ),
        )
            .into_response());
    }
    match state.store.authenticate(&token).await? {
        Some(principal) => {
            // The token was checked once; from here on the browser holds only
            // an opaque server-side session id.
            let (session, _) = state.store.create_web_session(&principal).await?;
            let cookie = session_cookie(
                &state,
                &session,
                i64::try_from(crate::store::WEB_SESSION_TTL.as_secs()).unwrap_or(0),
            );
            let mut response = Redirect::to("/shop").into_response();
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie)
                    .map_err(|error| ApiError::Internal(error.to_string()))?,
            );
            Ok(response)
        }
        None => Ok((
            StatusCode::UNAUTHORIZED,
            page(
                "Sign in",
                &Viewer::anonymous(),
                "<p>That token was not accepted.</p>",
            ),
        )
            .into_response()),
    }
}

/// Sign out: the server-side session is revoked, so a copied cookie is
/// useless afterwards, and the browser's cookie is cleared.
async fn logout(
    State(state): State<RegistryState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    same_site_post(&headers)?;
    if let Some(session) = cookie_session(&headers) {
        state.store.revoke_web_session(&session).await?;
    }
    let mut response = Redirect::to("/shop").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&state, "", 0))
            .map_err(|error| ApiError::Internal(error.to_string()))?,
    );
    Ok(response)
}

pub fn shop_router(state: RegistryState) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::to("/shop") }))
        .route("/shop", get(catalog))
        .route("/shop/skills/{id}", get(detail))
        .route("/shop/how-to-use", get(how_to_use))
        .route("/shop/login", get(login_form).post(login))
        .route("/shop/logout", post(logout))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_neutralises_markup() {
        assert_eq!(
            escape("<script>alert('x') & \"y\"</script>"),
            "&lt;script&gt;alert(&#39;x&#39;) &amp; &quot;y&quot;&lt;/script&gt;"
        );
        assert_eq!(short("abcdef", 3), "abc…");
    }
}
