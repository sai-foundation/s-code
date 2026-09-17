//! Provider discovery shared by the terminal and local Web setup.
use super::*;
use s_code_config::onboarding::{self, SavedProvider};
use std::path::PathBuf;

pub const PROMOTIONS_URL: &str = "https://api.sai.foundation/api/public/promotions";
#[derive(Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub protocol: &'static str,
    pub base_url: &'static str,
    pub key_url: &'static str,
    pub requires_key: bool,
}
pub fn presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "sai",
            name: "SAI",
            protocol: "openai_compatible",
            base_url: "https://api.sai.foundation/v1",
            key_url: "https://api.sai.foundation/",
            requires_key: true,
        },
        Preset {
            id: "openai",
            name: "OpenAI",
            protocol: "openai_compatible",
            base_url: "https://api.openai.com/v1",
            key_url: "https://platform.openai.com/api-keys",
            requires_key: true,
        },
        Preset {
            id: "anthropic",
            name: "Claude",
            protocol: "anthropic",
            base_url: "https://api.anthropic.com/v1",
            key_url: "https://console.anthropic.com/settings/keys",
            requires_key: true,
        },
        Preset {
            id: "gemini",
            name: "Gemini",
            protocol: "gemini",
            base_url: "https://generativelanguage.googleapis.com/v1beta",
            key_url: "https://aistudio.google.com/apikey",
            requires_key: true,
        },
        Preset {
            id: "deepseek",
            name: "DeepSeek",
            protocol: "openai_compatible",
            base_url: "https://api.deepseek.com/v1",
            key_url: "https://platform.deepseek.com/api_keys",
            requires_key: true,
        },
        Preset {
            id: "openrouter",
            name: "OpenRouter",
            protocol: "openai_compatible",
            base_url: "https://openrouter.ai/api/v1",
            key_url: "https://openrouter.ai/settings/keys",
            requires_key: true,
        },
        Preset {
            id: "local",
            name: "Local",
            protocol: "openai_compatible",
            base_url: "http://127.0.0.1:11434/v1",
            key_url: "",
            requires_key: false,
        },
        Preset {
            id: "openai-compatible",
            name: "Custom",
            protocol: "openai_compatible",
            base_url: "",
            key_url: "",
            requires_key: false,
        },
    ]
}

// No Debug implementation: the key must not appear in tracing or errors.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub preset: String,
    pub base_url: String,
    pub api_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    pub connection: Connection,
    pub model: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
}

pub fn valid_model(value: &str) -> bool {
    // Model identifiers are later persisted in non-secret session settings.
    let lower = value.to_ascii_lowercase();
    if lower.contains('=')
        || lower.contains("authorization:")
        || lower
            .strip_prefix("sk-")
            .is_some_and(|suffix| suffix.len() >= 16)
    {
        return false;
    }
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}
pub fn validate_base_url(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "Enter a valid API endpoint")?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
    {
        return Err(
            "Use HTTPS, or HTTP on localhost, without credentials or query parameters".into(),
        );
    }
    Ok(())
}
impl Connection {
    pub fn validate(&self) -> Result<Preset, String> {
        let preset = presets()
            .into_iter()
            .find(|p| p.id == self.preset)
            .ok_or("Unknown provider")?;
        validate_base_url(&self.base_url)?;
        if !matches!(preset.id, "local" | "openai-compatible")
            && self.base_url.trim_end_matches('/') != preset.base_url
        {
            return Err("Use Custom for a different API endpoint".into());
        }
        if self.api_key.len() > 8192
            || self
                .api_key
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("API key contains invalid characters or is too long".into());
        }
        if preset.requires_key && self.api_key.is_empty() {
            return Err("Enter your API key".into());
        }
        Ok(preset)
    }
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot start connection check".into())
}
async fn bounded_json(mut response: reqwest::Response) -> Result<Value, String> {
    match response.status().as_u16() {
        200..=299 => {}
        401 | 403 => return Err("The provider rejected this API key or its permissions".into()),
        429 => return Err("The provider is busy or rate limited; try again shortly".into()),
        _ => {
            return Err(format!(
                "The provider returned HTTP {}",
                response.status().as_u16()
            ));
        }
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Could not read the provider response")?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err("Provider response is too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Provider returned an invalid response".into())
}
pub async fn discover(connection: &Connection) -> Result<Vec<DiscoveredModel>, String> {
    let preset = connection.validate()?;
    let http = client()?;
    // OpenRouter's model catalog is public; check the account key separately.
    if preset.id == "openrouter" {
        let response = http
            .get(format!(
                "{}/auth/key",
                connection.base_url.trim_end_matches('/')
            ))
            .bearer_auth(&connection.api_key)
            .send()
            .await
            .map_err(|_| "Cannot verify the OpenRouter API key")?;
        bounded_json(response).await?;
    }
    let mut request = http.get(format!(
        "{}/models",
        connection.base_url.trim_end_matches('/')
    ));
    match preset.protocol {
        "anthropic" => {
            request = request
                .header("x-api-key", &connection.api_key)
                .header("anthropic-version", "2023-06-01")
                .query(&[("limit", "1000")]);
        }
        "gemini" => {
            request = request
                .header("x-goog-api-key", &connection.api_key)
                .query(&[("pageSize", "1000")]);
        }
        _ if !connection.api_key.is_empty() => {
            request = request.bearer_auth(&connection.api_key);
        }
        _ => {}
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Cannot reach the API endpoint; check the address and your connection")?;
    let json = bounded_json(response).await?;
    let entries = json
        .get(if preset.protocol == "gemini" {
            "models"
        } else {
            "data"
        })
        .and_then(Value::as_array)
        .ok_or("Provider did not return a model list")?;
    let mut models = BTreeMap::new();
    for entry in entries.iter().take(2048) {
        if preset.protocol == "gemini"
            && !entry
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| {
                    methods
                        .iter()
                        .any(|m| m.as_str() == Some("generateContent"))
                })
        {
            continue;
        }
        let id = entry
            .get(if preset.protocol == "gemini" {
                "name"
            } else {
                "id"
            })
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_start_matches("models/");
        if !valid_model(id) {
            continue;
        }
        let name = entry
            .get("display_name")
            .or_else(|| entry.get("displayName"))
            .or_else(|| entry.get("name"))
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 256 && !s.chars().any(char::is_control))
            .unwrap_or(id);
        models.insert(
            id.to_owned(),
            DiscoveredModel {
                id: id.into(),
                name: name.into(),
            },
        );
    }
    if models.is_empty() {
        return Err("No usable models were returned for this account".into());
    }
    Ok(models.into_values().collect())
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Promotion {
    pub id: String,
    pub title: String,
    pub description: String,
    pub terms: String,
    pub url: String,
    pub starts_at: String,
    pub ends_at: String,
}
pub async fn promotions() -> Vec<Promotion> {
    async fn fetch() -> Result<Vec<Promotion>, String> {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Client unavailable")?
            .get(PROMOTIONS_URL)
            .send()
            .await
            .map_err(|_| "Offers unavailable")?;
        let json = bounded_json(response).await?;
        if json.get("schema_version").and_then(Value::as_u64) != Some(1) {
            return Ok(vec![]);
        }
        let offers: Vec<Promotion> =
            serde_json::from_value(json["promotions"].clone()).map_err(|_| "Invalid offers")?;
        let now = chrono::Utc::now();
        Ok(offers
            .into_iter()
            .filter(|p| {
                let active = chrono::DateTime::parse_from_rfc3339(&p.starts_at)
                    .is_ok_and(|t| t <= now)
                    && chrono::DateTime::parse_from_rfc3339(&p.ends_at).is_ok_and(|t| t > now);
                let valid_link = url::Url::parse(&p.url).is_ok_and(|u| {
                    u.origin().ascii_serialization() == "https://api.sai.foundation"
                        && u.username().is_empty()
                        && u.password().is_none()
                });
                active
                    && valid_link
                    && p.title.len() <= 100
                    && p.description.len() <= 500
                    && p.terms.len() <= 1000
                    && [&p.title, &p.description, &p.terms]
                        .iter()
                        .all(|s| !s.chars().any(char::is_control))
            })
            .take(8)
            .collect())
    }
    fetch().await.unwrap_or_default()
}

struct PrivateCredential(String);
#[async_trait]
impl CredentialProvider for PrivateCredential {
    async fn resolve(&self, _: &str) -> Result<String, GatewayError> {
        Ok(self.0.clone())
    }
}
pub fn saved_provider(saved: SavedProvider) -> Result<Arc<dyn ModelProvider>, String> {
    validate_base_url(&saved.base_url)?;
    if !valid_model(&saved.model) {
        return Err("Invalid saved model".into());
    }
    let credentials = Arc::new(PrivateCredential(saved.api_key.clone()));
    Ok(match saved.provider.as_str() {
        "anthropic" => Arc::new(AnthropicMessages::new(
            saved.base_url,
            "saved-provider",
            credentials,
        )),
        "gemini" => Arc::new(GeminiGenerateContent::new(
            saved.base_url,
            "saved-provider",
            credentials,
        )),
        "openai_compatible" if saved.api_key.is_empty() => {
            Arc::new(OpenAiCompatible::without_auth(saved.base_url, credentials))
        }
        "openai_compatible" => Arc::new(OpenAiCompatible::new(
            saved.base_url,
            "saved-provider",
            credentials,
        )),
        _ => return Err("Invalid saved provider protocol".into()),
    })
}

// Resolve once per model request: existing streams keep their provider.
// This also lets CLI setup update an already-running local service without a restart.
pub struct LocalProvider {
    pub path: PathBuf,
    pub needs_setup: bool,
    pub fallback: Option<Arc<dyn ModelProvider>>,
    pub fallback_credentials_available: bool,
}
impl LocalProvider {
    pub fn configured(&self) -> bool {
        onboarding::read(&self.path).ok().flatten().is_some() || self.fallback.is_some()
    }
    pub fn credentials_available(&self) -> bool {
        onboarding::read(&self.path).ok().flatten().is_some() || self.fallback_credentials_available
    }
}
#[async_trait]
impl ModelProvider for LocalProvider {
    async fn stream(&self, request: ModelRequest) -> Result<ModelStream, GatewayError> {
        let provider = match onboarding::read(&self.path).map_err(GatewayError::Credential)? {
            Some(saved) => saved_provider(saved).map_err(GatewayError::Credential)?,
            None => self.fallback.clone().ok_or_else(|| {
                GatewayError::Credential("Connect a model provider in Settings".into())
            })?,
        };
        provider.stream(request).await
    }
}

pub async fn save_setup(path: &std::path::Path, request: &SetupRequest) -> Result<(), String> {
    let preset = request.connection.validate()?;
    if !valid_model(&request.model) {
        return Err("Choose a valid model ID".into());
    }
    // Validate again at commit time; no stale UI check can authorize another key/endpoint.
    let models = discover(&request.connection).await?;
    if !models.iter().any(|m| m.id == request.model) {
        return Err("Choose a model available to this API key".into());
    }
    let saved = SavedProvider {
        provider: preset.protocol.into(),
        base_url: request.connection.base_url.trim_end_matches('/').into(),
        api_key: request.connection.api_key.clone(),
        model: request.model.clone(),
    };
    saved_provider(saved.clone())?;
    onboarding::save(path, &saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    async fn server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, task)
    }
    #[tokio::test]
    async fn discovery_authenticates_filters_and_does_not_echo_provider_errors() {
        let (base_url, task) = server(Router::new().route("/v1/models", get(|headers: HeaderMap| async move {
            if headers.get("authorization").and_then(|h| h.to_str().ok()) != Some("Bearer unit-private-key") {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "error": "echo-sensitive-value" })));
            }
            (StatusCode::OK, Json(serde_json::json!({"data":[{"id":"code-model"},{"id":"bad\nmodel"},{"id":"code-model"}]})))
        }))).await;
        let connection = Connection {
            preset: "openai-compatible".into(),
            base_url,
            api_key: "unit-private-key".into(),
        };
        let models = discover(&connection).await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "code-model");
        let wrong = Connection {
            api_key: "incorrect".into(),
            ..connection
        };
        let error = discover(&wrong).await.err().unwrap();
        assert!(error.contains("rejected"));
        assert!(!error.contains("echo-sensitive"));
        task.abort();
    }
    #[tokio::test]
    async fn redirects_do_not_forward_keys() {
        let (base_url, task) = server(Router::new().route(
            "/v1/models",
            get(|| async { axum::response::Redirect::temporary("https://example.invalid/stolen") }),
        ))
        .await;
        let error = discover(&Connection {
            preset: "openai-compatible".into(),
            base_url,
            api_key: "unit-private-key".into(),
        })
        .await
        .err()
        .unwrap();
        assert!(error.contains("307"));
        task.abort();
    }
    #[test]
    fn presets_cannot_redirect_keys_and_custom_rejects_unsafe_urls() {
        assert_eq!(presets()[0].id, "sai");
        let connection = Connection {
            preset: "sai".into(),
            base_url: "https://another.example/v1".into(),
            api_key: "key".into(),
        };
        assert!(connection.validate().is_err());
        for url in [
            "http://example.com/v1",
            "https://user:password@example.com",
            "https://example.com?key=private",
            "file:///tmp/a",
        ] {
            assert!(validate_base_url(url).is_err());
        }
        assert!(validate_base_url("http://[::1]:11434/v1").is_ok());
    }
    #[tokio::test]
    async fn save_rechecks_model_before_replacing_credentials() {
        let (base_url, task) = server(Router::new().route(
            "/v1/models",
            get(|| async { Json(serde_json::json!({"data":[{"id":"available"}]})) }),
        ))
        .await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provider.json");
        let request = SetupRequest {
            connection: Connection {
                preset: "local".into(),
                base_url,
                api_key: String::new(),
            },
            model: "available".into(),
        };
        save_setup(&path, &request).await.unwrap();
        let original = std::fs::read(&path).unwrap();
        let invalid = SetupRequest {
            model: "unavailable".into(),
            ..request
        };
        assert!(save_setup(&path, &invalid).await.is_err());
        assert_eq!(std::fs::read(path).unwrap(), original);
        task.abort();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn an_existing_runtime_uses_new_credentials_without_restart() {
        let (base_url, task) = server(Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|headers: HeaderMap| async move {
                assert_eq!(
                    headers.get("authorization").unwrap(),
                    "Bearer unit-private-key"
                );
                "data: {\"choices\":[{\"delta\":{\"content\":\"Connected\"}}]}\n\ndata: [DONE]\n\n"
            }),
        ))
        .await;
        let directory = tempfile::tempdir().unwrap();
        let runtime = LocalProvider {
            path: directory.path().join("provider.json"),
            fallback: None,
            fallback_credentials_available: false,
            needs_setup: true,
        };
        assert!(!runtime.configured());
        onboarding::save(
            &runtime.path,
            &SavedProvider {
                provider: "openai_compatible".into(),
                base_url,
                api_key: "unit-private-key".into(),
                model: "coding-model".into(),
            },
        )
        .unwrap();
        assert!(runtime.configured());
        let mut stream = runtime
            .stream(ModelRequest {
                model: "coding-model".into(),
                temperature: 0.0,
                messages: vec![],
                tools: vec![],
                max_output_tokens: 16,
                routing: None,
            })
            .await
            .unwrap();
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let ModelEvent::TextDelta { text: delta } = event.unwrap() {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "Connected");
        task.abort();
    }
    #[test]
    fn sai_requires_a_user_key_independently_of_optional_offers() {
        let mut connection = Connection {
            preset: "sai".into(),
            base_url: "https://api.sai.foundation/v1".into(),
            api_key: String::new(),
        };
        assert_eq!(
            connection.validate().err().as_deref(),
            Some("Enter your API key")
        );
        connection.api_key = "synthetic-account-key".into();
        let preset = connection.validate().unwrap();
        assert!(preset.requires_key);
        assert_eq!(preset.base_url, "https://api.sai.foundation/v1");
    }
}
