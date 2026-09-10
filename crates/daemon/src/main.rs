use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use s_code_audit::{
    CentralAuditDataKeyMaterial, CentralAuditIngestReceipt, CentralAuditSigner,
    CentralAuditWrappedDataKey, HashChain, SignedCentralAuditBatch,
};
use s_code_config::{
    ClientConfig, Component, ConfigLoader, LocalDaemonConnection, ModelConfig, Profile,
    acquire_local_daemon_instance, publish_local_daemon_connection,
};
use s_code_connector_sdk::{
    ActionContext, CredentialBroker, EnvironmentCredentialBroker, GitHubActionsConnector,
    GitHubEnterpriseConnector, JiraWorkManagementConnector, ServiceNowConnector, SlackConnector,
    SplunkHecConnector, TeamsConnector,
};
use s_code_daemon::{
    AppState, CentralAuditDataKeyProvider, CentralAuditDelivery, CentralAuditExporter,
    ExperienceMode, StoreMcpOAuthAuthorizationProvider, app, community_mcp_permissions_sha256,
    community_plugin_permissions_sha256,
};
use s_code_identity::TeamGrantVerifier;
use s_code_mcp_client::{
    McpHttpAuthorizationProvider, McpHttpServerConfig, McpRegistry, McpServerConfig,
};
use s_code_model_gateway::{
    AnthropicMessages, EnvironmentCredentials, GeminiGenerateContent, GovernedModelRouter,
    ModelProvider, OpenAiCompatible, RoutedModelEndpoint,
};
use s_code_policy::PolicyTrustStore;
use s_code_protocol::{DaemonSettings, Id, Scope};
use s_code_storage::{StorageError, StorageFormat, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::future::IntoFuture;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use zeroize::Zeroize;

const LOCAL_MANAGED_STORAGE_KEY_ID: &str = "local-managed-v1";

enum StorageConnection {
    Encrypted {
        key: Vec<u8>,
        protection: &'static str,
    },
    Plaintext {
        protection: &'static str,
    },
}

fn managed_storage_key_path(database: &Path) -> Result<PathBuf, String> {
    let file_name = database
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("database path has no valid file name")?;
    Ok(database.with_file_name(format!(".{file_name}.storage-key")))
}

fn load_private_managed_key(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("managed storage key must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("managed storage key must use mode 0600".into());
        }
    }
    let key =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if key.len() != 32 {
        return Err("managed storage key must contain exactly 32 bytes".into());
    }
    Ok(key)
}

fn create_private_temporary_file(
    directory: &Path,
    prefix: &str,
) -> Result<(PathBuf, std::fs::File), String> {
    for _ in 0..32 {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce)
            .map_err(|error| format!("cannot generate temporary file name: {error}"))?;
        let temporary = directory.join(format!(".{prefix}.{}.tmp", URL_SAFE_NO_PAD.encode(nonce)));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("cannot create {}: {error}", temporary.display()));
            }
        }
    }
    Err("cannot allocate a unique private temporary file".into())
}

fn create_private_managed_key(path: &Path) -> Result<Vec<u8>, String> {
    let directory = path.parent().ok_or("managed storage key has no parent")?;
    create_private_directories(directory)?;
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("managed storage directory must be a regular directory".into());
    }
    let mut key = vec![0_u8; 32];
    getrandom::fill(&mut key).map_err(|error| format!("cannot generate storage key: {error}"))?;
    let (temporary, mut file) = match create_private_temporary_file(directory, "storage-key") {
        Ok(temporary) => temporary,
        Err(error) => {
            key.zeroize();
            return Err(error);
        }
    };
    let result = (|| -> Result<(), String> {
        file.write_all(&key)
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
        drop(file);
        match std::fs::hard_link(&temporary, path) {
            Ok(()) => {
                std::fs::remove_file(&temporary).map_err(|error| {
                    format!("cannot finish publishing {}: {error}", path.display())
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(&temporary).map_err(|remove_error| {
                    format!(
                        "cannot clean up raced key {}: {remove_error}",
                        temporary.display()
                    )
                })?;
                key.zeroize();
                key = load_private_managed_key(path)?;
            }
            Err(error) => {
                return Err(format!("cannot publish {}: {error}", path.display()));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        key.zeroize();
    }
    result?;
    Ok(key)
}

fn create_private_directories(directory: &Path) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut cursor = directory;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .ok_or("managed storage directory has no existing ancestor")?;
    }
    for path in missing.into_iter().rev() {
        match std::fs::create_dir(&path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                        .map_err(|error| format!("cannot secure {}: {error}", path.display()))?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
        }
    }
    Ok(())
}

fn replace_private_managed_key(path: &Path, key: &[u8]) -> Result<(), String> {
    if key.len() != 32 {
        return Err("managed storage key must contain exactly 32 bytes".into());
    }
    let directory = path.parent().ok_or("managed storage key has no parent")?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let (temporary, mut file) = create_private_temporary_file(directory, "storage-key.restore")?;
    let result = (|| -> Result<(), String> {
        file.write_all(key)
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
        drop(file);
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("cannot publish {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[derive(Serialize, Deserialize)]
struct RestoreJournal {
    replace_managed_key: bool,
    had_previous_database: bool,
    had_previous_key: bool,
    phase: String,
}

struct RestorePaths {
    marker: PathBuf,
    staged_database: PathBuf,
    previous_database: PathBuf,
    staged_key: PathBuf,
    previous_key: PathBuf,
    destination_key: PathBuf,
}

fn restore_paths(destination: &Path) -> Result<RestorePaths, String> {
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("restore destination has no valid file name")?;
    let parent = destination
        .parent()
        .ok_or("restore destination has no parent")?;
    Ok(RestorePaths {
        marker: parent.join(format!(".{file_name}.restore-pending.json")),
        staged_database: parent.join(format!(".{file_name}.restore-new")),
        previous_database: parent.join(format!(".{file_name}.restore-old")),
        staged_key: parent.join(format!(".{file_name}.storage-key.restore-new")),
        previous_key: parent.join(format!(".{file_name}.storage-key.restore-old")),
        destination_key: managed_storage_key_path(destination)?,
    })
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

fn recover_interrupted_restore(destination: &Path) -> Result<(), String> {
    let paths = restore_paths(destination)?;
    if !paths.marker.exists() {
        return Ok(());
    }
    let encoded = std::fs::read(&paths.marker)
        .map_err(|error| format!("cannot read restore journal: {error}"))?;
    let journal: RestoreJournal = serde_json::from_slice(&encoded)
        .map_err(|error| format!("invalid restore journal: {error}"))?;
    if journal.phase == "committed" {
        if !destination.exists() || (journal.replace_managed_key && !paths.destination_key.exists())
        {
            return Err(
                "committed restore journal is missing the published database or key".into(),
            );
        }
    } else if journal.phase == "publishing" {
        if journal.had_previous_database {
            if paths.previous_database.exists() {
                remove_file_if_present(destination)?;
                std::fs::rename(&paths.previous_database, destination).map_err(|error| {
                    format!(
                        "cannot roll back {} to {}: {error}",
                        paths.previous_database.display(),
                        destination.display()
                    )
                })?;
            } else if !paths.staged_database.exists() {
                return Err("restore journal lost the previous database copy".into());
            }
        } else if !paths.staged_database.exists() {
            remove_file_if_present(destination)?;
        }
        if journal.replace_managed_key && journal.had_previous_key && paths.previous_key.exists() {
            remove_file_if_present(&paths.destination_key)?;
            std::fs::rename(&paths.previous_key, &paths.destination_key).map_err(|error| {
                format!(
                    "cannot roll back {} to {}: {error}",
                    paths.previous_key.display(),
                    paths.destination_key.display()
                )
            })?;
        } else if journal.replace_managed_key
            && !journal.had_previous_key
            && !paths.staged_key.exists()
        {
            remove_file_if_present(&paths.destination_key)?;
        }
    } else {
        return Err("restore journal has an unknown phase".into());
    }
    remove_file_if_present(&paths.staged_database)?;
    remove_file_if_present(&paths.staged_key)?;
    remove_file_if_present(&paths.previous_database)?;
    remove_file_if_present(&paths.previous_key)?;
    remove_file_if_present(&paths.marker)?;
    Ok(())
}

fn publish_restored_database(destination: &Path, replace_managed_key: bool) -> Result<(), String> {
    let paths = restore_paths(destination)?;
    let mut journal = RestoreJournal {
        replace_managed_key,
        had_previous_database: destination.exists(),
        had_previous_key: paths.destination_key.exists(),
        phase: "publishing".into(),
    };
    write_private_json(&paths.marker, &journal)?;
    let result = (|| -> Result<(), String> {
        if destination.exists() {
            std::fs::rename(destination, &paths.previous_database)
                .map_err(|error| format!("cannot preserve current database: {error}"))?;
        }
        if replace_managed_key && paths.destination_key.exists() {
            std::fs::rename(&paths.destination_key, &paths.previous_key)
                .map_err(|error| format!("cannot preserve current managed key: {error}"))?;
        }
        std::fs::rename(&paths.staged_database, destination)
            .map_err(|error| format!("cannot publish restored database: {error}"))?;
        if replace_managed_key {
            std::fs::rename(&paths.staged_key, &paths.destination_key)
                .map_err(|error| format!("cannot publish restored managed key: {error}"))?;
        }
        journal.phase = "committed".into();
        write_private_json(&paths.marker, &journal)?;
        Ok(())
    })();
    if let Err(error) = result {
        recover_interrupted_restore(destination)?;
        return Err(error);
    }
    remove_file_if_present(&paths.previous_database)?;
    remove_file_if_present(&paths.previous_key)?;
    remove_file_if_present(&paths.marker)?;
    Ok(())
}

async fn resolve_storage_connection(
    database_url: &str,
    explicit_key_id: Option<&str>,
    explicit_key_base64: Option<&str>,
) -> Result<StorageConnection, String> {
    if let (Some(_), Some(encoded_key)) = (explicit_key_id, explicit_key_base64) {
        let key = URL_SAFE_NO_PAD
            .decode(encoded_key)
            .map_err(|error| format!("invalid storage encryption key: {error}"))?;
        return Ok(StorageConnection::Encrypted {
            key,
            protection: "explicit_encrypted",
        });
    }
    if database_url == "sqlite::memory:" {
        return Ok(StorageConnection::Plaintext {
            protection: "ephemeral_memory",
        });
    }
    let database = database_path(database_url)?;
    let key_path = managed_storage_key_path(&database)?;
    let key_exists = match std::fs::symlink_metadata(&key_path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", key_path.display()));
        }
    };
    let database_has_content = match std::fs::symlink_metadata(&database) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("SQLite database path must be a regular file".into());
        }
        Ok(metadata) => metadata.len() > 0,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", database.display()));
        }
    };
    let format = if key_exists {
        Store::inspect_storage_format(database_url).await
    } else {
        Store::inspect_storage_format_without_side_effects(database_url).await
    }
    .map_err(|error| format!("cannot inspect storage encryption metadata: {error}"))?;
    if key_exists {
        if matches!(format, StorageFormat::Marked { ref key_id } if key_id != LOCAL_MANAGED_STORAGE_KEY_ID)
        {
            return Err(
                "database storage metadata does not match the local managed key; provide the configured explicit key"
                    .into(),
            );
        }
        return Ok(StorageConnection::Encrypted {
            key: load_private_managed_key(&key_path)?,
            protection: "managed_encrypted",
        });
    }
    match format {
        StorageFormat::Marked { key_id } if key_id == "plaintext-v1" => {
            return Ok(StorageConnection::Plaintext {
                protection: "legacy_plaintext",
            });
        }
        StorageFormat::Marked { key_id } => {
            return Err(format!(
                "database requires storage key {key_id}, but its managed key file is missing"
            ));
        }
        StorageFormat::Unmarked => {
            return Err(
                "existing database has no storage encryption marker; refusing to open it implicitly"
                    .into(),
            );
        }
        StorageFormat::Empty if !database_has_content => {}
        StorageFormat::Empty => {
            return Err(
                "existing database is not safely inspectable and its managed key file is missing"
                    .into(),
            );
        }
    }
    Ok(StorageConnection::Encrypted {
        key: create_private_managed_key(&key_path)?,
        protection: "managed_encrypted",
    })
}

async fn connect_resolved_store(
    database_url: &str,
    explicit_key_id: Option<&str>,
    explicit_key_base64: Option<&str>,
) -> Result<(Store, &'static str), Box<dyn std::error::Error>> {
    let connection =
        resolve_storage_connection(database_url, explicit_key_id, explicit_key_base64).await?;
    match connection {
        StorageConnection::Encrypted {
            mut key,
            protection,
        } => {
            let key_id = explicit_key_id.unwrap_or(LOCAL_MANAGED_STORAGE_KEY_ID);
            let result = Store::connect_encrypted(database_url, key_id, &key).await;
            key.zeroize();
            Ok((result?, protection))
        }
        StorageConnection::Plaintext { protection } => {
            Ok((Store::connect(database_url).await?, protection))
        }
    }
}

struct HttpCentralAuditDelivery {
    client: reqwest::Client,
    ingest_url: String,
}

impl HttpCentralAuditDelivery {
    fn new(ingest_url: String) -> Result<Self, Box<dyn std::error::Error>> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("s-code-daemon/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { client, ingest_url })
    }
}

async fn bounded_http_response_bytes(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> anyhow::Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        anyhow::bail!("{label} exceeds {limit} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > limit {
            body.zeroize();
            anyhow::bail!("{label} exceeds {limit} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[async_trait::async_trait]
impl CentralAuditDelivery for HttpCentralAuditDelivery {
    async fn deliver(
        &self,
        envelope: &SignedCentralAuditBatch,
    ) -> anyhow::Result<CentralAuditIngestReceipt> {
        let response = self
            .client
            .post(&self.ingest_url)
            .json(envelope)
            .send()
            .await?;
        if !response.status().is_success() {
            anyhow::bail!(
                "central audit ingestion returned HTTP {}",
                response.status()
            );
        }
        let body =
            bounded_http_response_bytes(response, 64 * 1024, "central audit ingestion receipt")
                .await?;
        Ok(serde_json::from_slice(&body)?)
    }
}

#[derive(Serialize)]
struct GenerateAuditDataKeyRequest<'a> {
    organization_id: &'a Id,
    team_id: &'a Id,
    batch_id: &'a Id,
    key_id: &'a str,
    purpose: &'static str,
}

#[derive(Deserialize)]
struct GenerateAuditDataKeyResponse {
    plaintext_data_key_base64: String,
    wrapped_data_key: CentralAuditWrappedDataKey,
}

struct HttpCentralAuditDataKeyProvider {
    client: reqwest::Client,
    generate_data_key_url: String,
    credential_handle: String,
    credential_broker: std::sync::Arc<dyn CredentialBroker>,
}

impl HttpCentralAuditDataKeyProvider {
    fn new(
        generate_data_key_url: String,
        credential_handle: String,
        credential_broker: std::sync::Arc<dyn CredentialBroker>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("s-code-daemon/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            generate_data_key_url,
            credential_handle,
            credential_broker,
        })
    }
}

#[async_trait::async_trait]
impl CentralAuditDataKeyProvider for HttpCentralAuditDataKeyProvider {
    async fn generate_data_key(
        &self,
        organization_id: &Id,
        team_id: &Id,
        batch_id: &Id,
        kms_key_id: &str,
    ) -> anyhow::Result<CentralAuditDataKeyMaterial> {
        let mut credential = self
            .credential_broker
            .issue(
                &self.credential_handle,
                &ActionContext {
                    scope: Scope {
                        organization_id: organization_id.clone(),
                        team_id: team_id.clone(),
                        actor_id: Id("machine-central-audit".into()),
                        goal_id: None,
                        task_id: Some(batch_id.clone()),
                    },
                    session_id: batch_id.clone(),
                    task_id: batch_id.clone(),
                    actor_reason: "Generate a scoped central-audit content data key".into(),
                    idempotency_key: batch_id.0.clone(),
                },
            )
            .await?;
        let response = self
            .client
            .post(&self.generate_data_key_url)
            .bearer_auth(&credential)
            .json(&GenerateAuditDataKeyRequest {
                organization_id,
                team_id,
                batch_id,
                key_id: kms_key_id,
                purpose: "s-code.central-audit.content.v1",
            })
            .send()
            .await;
        credential.zeroize();
        let response = response?;
        if !response.status().is_success() {
            anyhow::bail!(
                "central audit KMS GenerateDataKey returned HTTP {}",
                response.status()
            );
        }
        let mut body =
            bounded_http_response_bytes(response, 64 * 1024, "central audit KMS response").await?;
        let parsed = serde_json::from_slice(&body);
        body.zeroize();
        let mut response: GenerateAuditDataKeyResponse = parsed?;
        if response.wrapped_data_key.key_id != kms_key_id {
            anyhow::bail!("central audit KMS returned the wrong key id");
        }
        let mut plaintext = URL_SAFE_NO_PAD.decode(&response.plaintext_data_key_base64)?;
        response.plaintext_data_key_base64.zeroize();
        let material = CentralAuditDataKeyMaterial::new(&plaintext, response.wrapped_data_key);
        plaintext.zeroize();
        material
    }
}

fn configured_model_provider(
    provider: &str,
    base_url: String,
    credential_handle: Option<String>,
    credentials: std::sync::Arc<EnvironmentCredentials>,
) -> std::sync::Arc<dyn ModelProvider> {
    match provider {
        "openai_compatible" => match credential_handle {
            Some(handle) => {
                std::sync::Arc::new(OpenAiCompatible::new(base_url, handle, credentials))
            }
            None => std::sync::Arc::new(OpenAiCompatible::without_auth(base_url, credentials)),
        },
        "anthropic" => std::sync::Arc::new(AnthropicMessages::new(
            base_url,
            credential_handle.expect("native provider credential handle"),
            credentials,
        )),
        "gemini" => std::sync::Arc::new(GeminiGenerateContent::new(
            base_url,
            credential_handle.expect("native provider credential handle"),
            credentials,
        )),
        _ => unreachable!("validated model provider"),
    }
}

fn default_model_credential_handle(provider: &str) -> String {
    match provider {
        "anthropic" => "ANTHROPIC_API_KEY".into(),
        "gemini" => "GEMINI_API_KEY".into(),
        _ => "OPENAI_API_KEY".into(),
    }
}

fn model_credential_handle(
    provider: &str,
    base_url: &str,
    configured: Option<String>,
) -> Option<String> {
    configured.or_else(|| {
        if provider == "openai_compatible" && model_api_is_loopback(base_url) {
            None
        } else {
            Some(default_model_credential_handle(provider))
        }
    })
}

fn model_api_is_loopback(base_url: &str) -> bool {
    url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
}

fn credential_handle_is_available(handle: Option<String>) -> bool {
    handle.is_none_or(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn model_credentials_are_available(config: &ModelConfig) -> bool {
    if !config.endpoints.is_empty() {
        return config.endpoints.iter().all(|endpoint| {
            credential_handle_is_available(model_credential_handle(
                &endpoint.provider,
                &endpoint.base_url,
                endpoint.credential_handle.clone(),
            ))
        });
    }
    config.base_url.as_deref().is_some_and(|base_url| {
        credential_handle_is_available(model_credential_handle(
            &config.provider,
            base_url,
            config.credential_handle.clone(),
        ))
    })
}

#[derive(Serialize)]
struct DiagnosticDatabase {
    integrity: &'static str,
    event_sequence: u64,
    visible_session_count: usize,
}

#[derive(Serialize)]
struct DiagnosticConfiguration {
    telemetry_enabled: bool,
    max_context_tokens: u32,
    default_model_configured: bool,
}

fn accessible_directory_uri(uri: &str) -> bool {
    url::Url::parse(uri)
        .ok()
        .and_then(|value| value.to_file_path().ok())
        .and_then(|path| path.canonicalize().ok())
        .is_some_and(|path| path.is_dir())
}

async fn repair_development_settings(
    store: &Store,
    profile: Profile,
    client: &ClientConfig,
) -> Result<bool, Box<dyn std::error::Error>> {
    if profile != Profile::Development {
        return Ok(false);
    }
    let mut settings = store.get_settings().await?;
    let workspace_is_accessible = accessible_directory_uri(&settings.workspace_uri);
    let legacy_default_model = matches!(
        settings.default_model.as_str(),
        "gpt-5" | "openai/gpt-oss-20b"
    );
    let should_update_model = legacy_default_model && settings.default_model != client.model;
    if workspace_is_accessible && !should_update_model {
        return Ok(false);
    }
    if !workspace_is_accessible {
        let workspace_uri = client
            .workspace
            .as_deref()
            .filter(|uri| accessible_directory_uri(uri))
            .map(str::to_owned)
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|path| path.canonicalize().ok())
                    .filter(|path| path.is_dir())
                    .and_then(|path| url::Url::from_directory_path(path).ok())
                    .map(Into::into)
            })
            .ok_or("no accessible development workspace is configured")?;
        settings.organization_id = Id(client.organization_id.clone());
        settings.team_id = Id(client.team_id.clone());
        settings.actor_id = Id(client.actor_id.clone());
        settings.workspace_uri = workspace_uri;
        settings.default_model = client.model.clone();
    } else if should_update_model {
        settings.default_model = client.model.clone();
    }
    store.put_settings(&settings).await?;
    Ok(true)
}

#[derive(Serialize)]
struct DiagnosticBundle {
    schema_version: u32,
    generated_at: chrono::DateTime<chrono::Utc>,
    daemon_version: &'static str,
    protocol_version: &'static str,
    operating_system: &'static str,
    architecture: &'static str,
    database: DiagnosticDatabase,
    configuration: DiagnosticConfiguration,
    content_included: bool,
    identifiers_included: bool,
}

fn write_private_json<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "destination requires a valid file name".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", ulid::Ulid::new()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut file, value).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

async fn build_diagnostic_bundle(store: &Store) -> Result<DiagnosticBundle, String> {
    store
        .verify_integrity()
        .await
        .map_err(|error| error.to_string())?;
    let settings = store
        .get_settings()
        .await
        .map_err(|error| error.to_string())?;
    let visible_session_count = store
        .list_sessions(&Scope {
            organization_id: settings.organization_id.clone(),
            team_id: settings.team_id.clone(),
            actor_id: settings.actor_id.clone(),
            goal_id: None,
            task_id: None,
        })
        .await
        .map_err(|error| error.to_string())?
        .len();
    Ok(DiagnosticBundle {
        schema_version: 1,
        generated_at: chrono::Utc::now(),
        daemon_version: env!("CARGO_PKG_VERSION"),
        protocol_version: s_code_protocol::PROTOCOL_VERSION,
        operating_system: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        database: DiagnosticDatabase {
            integrity: "ok",
            event_sequence: store
                .max_event_sequence()
                .await
                .map_err(|error| error.to_string())?,
            visible_session_count,
        },
        configuration: DiagnosticConfiguration {
            telemetry_enabled: settings.telemetry_enabled,
            max_context_tokens: settings.max_context_tokens,
            default_model_configured: !settings.default_model.is_empty(),
        },
        content_included: false,
        identifiers_included: false,
    })
}

fn read_configuration(path: &std::path::Path) -> Result<DaemonSettings, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > 1024 * 1024 {
        return Err("configuration exceeds 1 MiB".into());
    }
    let encoded = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&encoded).map_err(|error| error.to_string())
}

fn database_path(url: &str) -> Result<std::path::PathBuf, String> {
    if url == "sqlite::memory:" {
        return Err("in-memory database cannot be backed up or restored".into());
    }
    let value = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .ok_or_else(|| "backup and restore require a SQLite database URL".to_string())?;
    let value = value.split('?').next().unwrap_or_default();
    if value.is_empty() {
        return Err("SQLite database path is empty".into());
    }
    Ok(std::path::PathBuf::from(value))
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler must be installable");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

type McpSourceGroups = BTreeMap<String, (Vec<McpServerConfig>, Vec<McpHttpServerConfig>)>;

async fn connect_mcp_source_groups(
    store: &Store,
    extension_scope: &Scope,
    groups: McpSourceGroups,
    authorization: std::sync::Arc<dyn McpHttpAuthorizationProvider>,
) -> Result<
    (
        McpRegistry,
        Vec<(McpServerConfig, String)>,
        Vec<(McpHttpServerConfig, String)>,
    ),
    Box<dyn std::error::Error>,
> {
    let registry = McpRegistry::default();
    let mut active_mcp_sources = Vec::new();
    let mut active_mcp_http_sources = Vec::new();
    for (source, (configurations, http_configurations)) in groups {
        match McpRegistry::connect_with_http_authorization(
            &configurations,
            &http_configurations,
            Some(authorization.clone()),
        )
        .await
        {
            Ok(connected) => {
                registry.merge_from(&connected)?;
                active_mcp_sources.extend(
                    configurations
                        .into_iter()
                        .map(|configuration| (configuration, source.clone())),
                );
                active_mcp_http_sources.extend(
                    http_configurations
                        .into_iter()
                        .map(|configuration| (configuration, source.clone())),
                );
            }
            Err(error) if source.starts_with("s-code://extensions/") => {
                if let Some(server_id) = source.strip_prefix("s-code://extensions/mcp/") {
                    store.disable_mcp_server(extension_scope, server_id).await?;
                } else if let Some(server_id) = source.strip_prefix("s-code://extensions/mcp-http/")
                {
                    store
                        .disable_mcp_http_server(extension_scope, server_id)
                        .await?;
                } else if let Some(plugin_id) = source.strip_prefix("s-code://extensions/plugins/")
                {
                    let installation = store
                        .get_plugin_installation(extension_scope, plugin_id)
                        .await?;
                    store
                        .set_plugin_enabled(
                            extension_scope,
                            plugin_id,
                            false,
                            installation.revision,
                        )
                        .await?;
                }
                warn!(source = %source, ?error, "disabled unavailable MCP extension during recovery startup");
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok((registry, active_mcp_sources, active_mcp_http_sources))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(command) = args.first() {
        match command.as_str() {
            "--version" => {
                println!("s-code web {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--self-test" => {
                let store = Store::in_memory().await?;
                let _ = store.get_or_create_device_id().await?;
                println!("s-code web self-test ok");
                return Ok(());
            }
            "--help" | "-h" => {
                println!(
                    "s-code web [--version|--self-test|--verify-database|--backup PATH|--restore PATH|--export-config PATH|--import-config PATH|--diagnostics PATH|--config-validate|--config-print-effective|--config-explain FIELD]"
                );
                return Ok(());
            }
            _ => {}
        }
    }
    let effective = ConfigLoader::from_process().load(Component::Daemon)?;
    let database_url = effective.config.daemon.database_url.clone();
    if let Some(command) = args.first() {
        match command.as_str() {
            "--config-validate" => {
                println!(
                    "configuration is valid (schema v{})",
                    effective.config.schema_version
                );
                return Ok(());
            }
            "--config-print-effective" => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&effective.redacted_json())?
                );
                return Ok(());
            }
            "--config-explain" => {
                let path = args
                    .get(1)
                    .ok_or("--config-explain requires a field path")?;
                let provenance = effective
                    .provenance(path)
                    .ok_or("unknown configuration field")?;
                println!("{path}: {:?} ({})", provenance.source, provenance.detail);
                return Ok(());
            }
            "--verify-database" | "--backup" | "--restore" | "--export-config"
            | "--import-config" | "--diagnostics" => {}
            _ => return Err(format!("unknown argument: {command}").into()),
        }
    }
    let _database_instance = if database_url != "sqlite::memory:" {
        Some(acquire_local_daemon_instance()?)
    } else {
        None
    };
    if database_url != "sqlite::memory:" {
        recover_interrupted_restore(&database_path(&database_url)?)?;
    }
    let storage_key_id = effective.config.daemon.storage_encryption_key_id.as_deref();
    let storage_key_base64 = effective
        .config
        .daemon
        .storage_encryption_key_base64
        .as_deref();
    if let Some(command) = args.first() {
        match command.as_str() {
            "--verify-database" => {
                let (store, _) =
                    connect_resolved_store(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                store.verify_integrity().await?;
                println!("s-code database integrity ok");
                return Ok(());
            }
            "--backup" => {
                let destination = args.get(1).ok_or("--backup requires a destination path")?;
                let destination = std::path::Path::new(destination);
                let (store, protection) =
                    connect_resolved_store(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                if protection == "managed_encrypted" {
                    let destination_key = managed_storage_key_path(destination)?;
                    if std::fs::symlink_metadata(&destination_key).is_ok() {
                        return Err(format!(
                            "backup key destination already exists: {}",
                            destination_key.display()
                        )
                        .into());
                    }
                }
                store.backup_database(destination).await?;
                if protection == "managed_encrypted" {
                    let source_key = managed_storage_key_path(&database_path(&database_url)?)?;
                    let destination_key = managed_storage_key_path(destination)?;
                    let key = load_private_managed_key(&source_key)?;
                    if let Err(error) = replace_private_managed_key(&destination_key, &key) {
                        let _ = std::fs::remove_file(destination);
                        return Err(error.into());
                    }
                }
                println!("backup created at {}", destination.display());
                return Ok(());
            }
            "--restore" => {
                let backup = args.get(1).ok_or("--restore requires a backup path")?;
                let backup = std::path::Path::new(backup);
                let destination = database_path(&database_url)?;
                let backup_key_path = managed_storage_key_path(backup)?;
                let backup_url = format!("sqlite://{}", backup.display());
                let backup_format = Store::inspect_storage_format(&backup_url).await?;
                let destination_connection =
                    resolve_storage_connection(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                let (candidate_key_id, mut candidate_key, replace_managed_key) =
                    match (destination_connection, backup_format) {
                        (
                            StorageConnection::Encrypted {
                                key,
                                protection: "explicit_encrypted",
                            },
                            StorageFormat::Marked { key_id },
                        ) if Some(key_id.as_str()) == storage_key_id => {
                            if backup_key_path.exists() {
                                return Err(
                                    "explicit-key restore refuses a managed sibling key".into()
                                );
                            }
                            (key_id, Some(key), false)
                        }
                        (
                            StorageConnection::Encrypted {
                                mut key,
                                protection: "managed_encrypted",
                            },
                            StorageFormat::Marked { key_id },
                        ) if key_id == LOCAL_MANAGED_STORAGE_KEY_ID => {
                            key.zeroize();
                            if !backup_key_path.exists() {
                                return Err(
                                    "managed encrypted backup is missing its sibling key".into()
                                );
                            }
                            (
                                key_id,
                                Some(load_private_managed_key(&backup_key_path)?),
                                true,
                            )
                        }
                        (StorageConnection::Plaintext { .. }, StorageFormat::Marked { key_id })
                            if key_id == "plaintext-v1" =>
                        {
                            (key_id, None, false)
                        }
                        (StorageConnection::Encrypted { mut key, .. }, _) => {
                            key.zeroize();
                            return Err(
                                "backup encryption mode or key id does not match the destination"
                                    .into(),
                            );
                        }
                        _ => {
                            return Err(
                                "backup has no compatible, verified storage encryption marker"
                                    .into(),
                            );
                        }
                    };
                let paths = restore_paths(&destination)?;
                remove_file_if_present(&paths.staged_database)?;
                remove_file_if_present(&paths.staged_key)?;
                if paths.previous_database.exists()
                    || paths.previous_key.exists()
                    || paths.marker.exists()
                {
                    return Err("restore recovery artifacts require manual inspection".into());
                }
                let restore_result: Result<(), Box<dyn std::error::Error>> = async {
                    Store::restore_database(backup, &paths.staged_database).await?;
                    if replace_managed_key {
                        replace_private_managed_key(
                            &paths.staged_key,
                            candidate_key.as_deref().expect("managed restore key"),
                        )?;
                    }
                    let staged_url = format!("sqlite://{}", paths.staged_database.display());
                    let staged = if let Some(key) = candidate_key.as_deref() {
                        Store::connect_encrypted(&staged_url, &candidate_key_id, key).await?
                    } else {
                        Store::connect(&staged_url).await?
                    };
                    staged.verify_integrity().await?;
                    staged.audit_chain_records().await?;
                    staged.close().await;
                    publish_restored_database(&destination, replace_managed_key)?;
                    Ok(())
                }
                .await;
                if let Some(key) = candidate_key.as_mut() {
                    key.zeroize();
                }
                if let Err(error) = restore_result {
                    let _ = remove_file_if_present(&paths.staged_database);
                    let _ = remove_file_if_present(&paths.staged_key);
                    return Err(error);
                }
                println!("database restored from {}", backup.display());
                return Ok(());
            }
            "--export-config" => {
                let destination = args
                    .get(1)
                    .ok_or("--export-config requires a destination path")?;
                let (store, _) =
                    connect_resolved_store(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                let settings = store.get_settings().await?;
                write_private_json(std::path::Path::new(destination), &settings)?;
                println!("configuration exported to {destination}");
                return Ok(());
            }
            "--import-config" => {
                let source = args
                    .get(1)
                    .ok_or("--import-config requires a source path")?;
                let settings = read_configuration(std::path::Path::new(source))?;
                let (store, _) =
                    connect_resolved_store(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                store.put_settings(&settings).await?;
                println!("configuration imported from {source}");
                return Ok(());
            }
            "--diagnostics" => {
                let destination = args
                    .get(1)
                    .ok_or("--diagnostics requires a destination path")?;
                let (store, _) =
                    connect_resolved_store(&database_url, storage_key_id, storage_key_base64)
                        .await?;
                let bundle = build_diagnostic_bundle(&store).await?;
                write_private_json(std::path::Path::new(destination), &bundle)?;
                println!("content-free diagnostics exported to {destination}");
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {command}").into()),
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();
    let config = effective.config;
    let model_credentials_available = model_credentials_are_available(&config.model);
    let experience_mode = ExperienceMode::from_name(&config.daemon.experience_mode)?;
    let central_audit = config.daemon.central_audit.clone();
    let development_auth = config.daemon.auth_mode == "development_token";
    let token = config
        .daemon
        .token
        .clone()
        .unwrap_or_else(|| format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new()));
    let (store, storage_protection) = connect_resolved_store(
        &database_url,
        config.daemon.storage_encryption_key_id.as_deref(),
        config.daemon.storage_encryption_key_base64.as_deref(),
    )
    .await?;
    if storage_protection == "legacy_plaintext" {
        eprintln!("S_CODE_STORAGE_WARNING=legacy_plaintext; run `s-code doctor` for remediation");
    }
    if repair_development_settings(&store, config.profile, &config.client).await? {
        info!("repaired development settings");
    }
    let orphaned_background_terminals = store.mark_background_terminals_orphaned().await?;
    if orphaned_background_terminals > 0 {
        info!(
            count = orphaned_background_terminals,
            "marked unrecoverable background terminals as orphaned after daemon restart"
        );
    }
    let device_id = store.get_or_create_device_id().await?;
    let last_sequence = store.max_event_sequence().await?;
    let audit_records = store.audit_chain_records().await?;
    let verified_chain = HashChain::verify(
        audit_records
            .iter()
            .map(|(event, hash)| (event, hash.as_str())),
    )?;
    let chain_head = (!audit_records.is_empty()).then(|| verified_chain.head());
    let address = config.daemon.listen.parse::<SocketAddr>()?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    info!(%bound, "daemon listening on loopback");
    eprintln!("S_CODE_ADDR={bound}");
    if !development_auth {
        eprintln!("S_CODE_AUTH=team_grant");
    }
    let revoked_team_grants = store
        .active_remote_grant_revocations(chrono::Utc::now())
        .await?;
    let mut state = match chain_head {
        Some(head) => {
            AppState::new_with_chain_head(token.clone(), store.clone(), last_sequence, &head)
                .map_err(|error| format!("invalid persisted audit chain head: {error}"))?
        }
        None => AppState::new(token.clone(), store.clone(), last_sequence),
    }
    .with_device_id(device_id)
    .with_revoked_team_grants(revoked_team_grants)
    .with_model_credentials_available(model_credentials_available)
    .with_storage_protection(storage_protection)
    .with_experience_mode(experience_mode);
    let mut connector_approval_verifier = None;
    if !development_auth {
        let verifier = TeamGrantVerifier::from_base64(
            config
                .daemon
                .team_grant_key_id
                .as_deref()
                .expect("validated Team grant key id"),
            config
                .daemon
                .team_grant_public_key_base64
                .as_deref()
                .expect("validated Team grant public key"),
            &config.daemon.team_grant_issuer,
            &config.daemon.team_grant_audience,
        )?;
        connector_approval_verifier = Some(verifier.clone());
        state = state.with_team_grant_auth(verifier);
    }
    if let Some(runner_token) = config.daemon.runner_token {
        state = state.with_runner_token(runner_token);
    }
    if let (Some(key_id), Some(public_key)) = (
        config.daemon.team_config_key_id,
        config.daemon.team_config_public_key_base64,
    ) {
        let mut trust = PolicyTrustStore::default();
        trust.insert_base64(key_id, &public_key)?;
        state = state.with_team_configuration_trust(trust);
    }
    if central_audit.enabled {
        let kms_provider = match (
            central_audit.kms_generate_data_key_url.clone(),
            central_audit.kms_credential_handle.clone(),
        ) {
            (Some(url), Some(handle)) => {
                Some(std::sync::Arc::new(HttpCentralAuditDataKeyProvider::new(
                    url,
                    handle,
                    std::sync::Arc::new(EnvironmentCredentialBroker),
                )?))
            }
            (None, None) => None,
            _ => unreachable!("validated atomic central audit KMS configuration"),
        };
        let signer = CentralAuditSigner::from_base64(
            central_audit
                .key_id
                .as_deref()
                .expect("validated central audit key id"),
            central_audit
                .private_key_base64
                .as_deref()
                .expect("validated central audit private key"),
        )?;
        let delivery = std::sync::Arc::new(HttpCentralAuditDelivery::new(
            central_audit
                .ingest_url
                .clone()
                .expect("validated central audit ingestion URL"),
        )?);
        let mut exporter = CentralAuditExporter::new(
            store.clone(),
            signer,
            Id(central_audit
                .source_id
                .expect("validated central audit source id")),
            Id(central_audit
                .organization_id
                .expect("validated central audit Organization id")),
            Id(central_audit
                .team_id
                .expect("validated central audit Team id")),
            delivery,
        );
        if let Some(provider) = kms_provider {
            exporter = exporter.with_content_data_key_provider(provider);
        }
        state = state.with_central_audit_exporter(exporter);
    }
    let mut mcp_sources = Vec::<(McpServerConfig, String)>::new();
    let mut mcp_http_sources = Vec::<(McpHttpServerConfig, String)>::new();
    if development_auth && config.mcp.enabled {
        let path = config.mcp.config_path.expect("validated MCP config path");
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() > 1024 * 1024 {
            return Err("MCP configuration exceeds 1 MiB".into());
        }
        let configurations: Vec<McpServerConfig> = serde_json::from_slice(&std::fs::read(&path)?)?;
        let source_uri = url::Url::from_file_path(&path)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        mcp_sources.extend(
            configurations
                .into_iter()
                .map(|configuration| (configuration, source_uri.clone())),
        );
    }
    let extension_scope = {
        let settings = store.get_settings().await?;
        Scope {
            organization_id: settings.organization_id,
            team_id: settings.team_id,
            actor_id: settings.actor_id,
            goal_id: None,
            task_id: None,
        }
    };
    let mcp_installations = if development_auth {
        store.list_mcp_installations(&extension_scope).await?
    } else {
        Vec::new()
    };
    for installation in mcp_installations {
        if !installation.enabled {
            continue;
        }
        let current_permissions = community_mcp_permissions_sha256(&installation.server);
        if current_permissions.as_deref() != Ok(installation.permissions_sha256.as_str()) {
            store
                .disable_mcp_server(&extension_scope, &installation.server.id)
                .await?;
            warn!(
                server_id = %installation.server.id,
                "disabled MCP server because its executable identity changed; approve it again before use"
            );
            continue;
        }
        if mcp_sources.len() + mcp_http_sources.len() >= 32 {
            store
                .disable_mcp_server(&extension_scope, &installation.server.id)
                .await?;
            warn!(server_id = %installation.server.id, "disabled MCP extension because the 32-server runtime limit is full");
            continue;
        }
        if mcp_sources
            .iter()
            .any(|(configuration, _)| configuration.id == installation.server.id)
        {
            store
                .disable_mcp_server(&extension_scope, &installation.server.id)
                .await?;
            warn!(server_id = %installation.server.id, "disabled MCP extension because its server id conflicts with another source");
            continue;
        }
        mcp_sources.push((
            McpServerConfig {
                id: installation.server.id.clone(),
                program: installation.server.program,
                args: installation.server.args,
                environment_handles: installation.server.environment_handles,
                timeout_ms: installation.server.timeout_ms,
            },
            format!("s-code://extensions/mcp/{}", installation.server.id),
        ));
    }
    let mcp_http_installations = if development_auth {
        store.list_mcp_http_installations(&extension_scope).await?
    } else {
        Vec::new()
    };
    for installation in mcp_http_installations {
        if !installation.enabled {
            continue;
        }
        if installation.server.oauth.is_some() {
            match store
                .get_mcp_oauth_credential(&extension_scope, &installation.server.id)
                .await
            {
                Ok(_) => {}
                Err(StorageError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if mcp_sources.len() + mcp_http_sources.len() >= 32 {
            store
                .disable_mcp_http_server(&extension_scope, &installation.server.id)
                .await?;
            warn!(server_id = %installation.server.id, "disabled MCP HTTP extension because the 32-server runtime limit is full");
            continue;
        }
        if mcp_sources
            .iter()
            .any(|(configuration, _)| configuration.id == installation.server.id)
            || mcp_http_sources
                .iter()
                .any(|(configuration, _)| configuration.id == installation.server.id)
        {
            store
                .disable_mcp_http_server(&extension_scope, &installation.server.id)
                .await?;
            warn!(server_id = %installation.server.id, "disabled MCP HTTP extension because its server id conflicts with another source");
            continue;
        }
        mcp_http_sources.push((
            McpHttpServerConfig {
                id: installation.server.id.clone(),
                endpoint: installation.server.endpoint,
                header_handles: installation.server.header_handles,
                timeout_ms: installation.server.timeout_ms,
            },
            format!("s-code://extensions/mcp-http/{}", installation.server.id),
        ));
    }
    let plugin_installations = if development_auth {
        store.list_plugin_installations(&extension_scope).await?
    } else {
        Vec::new()
    };
    for installation in plugin_installations {
        if !installation.enabled {
            continue;
        }
        let current_permissions = community_plugin_permissions_sha256(&installation.bundle);
        if current_permissions.as_deref() != Ok(installation.bundle.permissions_sha256.as_str()) {
            store
                .set_plugin_enabled(
                    &extension_scope,
                    &installation.bundle.id,
                    false,
                    installation.revision,
                )
                .await?;
            warn!(
                plugin_id = %installation.bundle.id,
                "disabled Plugin because trusted host code changed; approve it again before use"
            );
            continue;
        }
        let plugin_server_count =
            installation.bundle.mcp_servers.len() + installation.bundle.mcp_http_servers.len();
        if mcp_sources.len() + mcp_http_sources.len() + plugin_server_count > 32 {
            store
                .set_plugin_enabled(
                    &extension_scope,
                    &installation.bundle.id,
                    false,
                    installation.revision,
                )
                .await?;
            warn!(plugin_id = %installation.bundle.id, "disabled Plugin because it exceeds the 32-server runtime limit");
            continue;
        }
        let conflicts = installation
            .bundle
            .mcp_servers
            .iter()
            .map(|server| &server.id)
            .chain(
                installation
                    .bundle
                    .mcp_http_servers
                    .iter()
                    .map(|server| &server.id),
            )
            .any(|server_id| {
                mcp_sources
                    .iter()
                    .any(|(configuration, _)| &configuration.id == server_id)
                    || mcp_http_sources
                        .iter()
                        .any(|(configuration, _)| &configuration.id == server_id)
            });
        if conflicts {
            store
                .set_plugin_enabled(
                    &extension_scope,
                    &installation.bundle.id,
                    false,
                    installation.revision,
                )
                .await?;
            warn!(plugin_id = %installation.bundle.id, "disabled Plugin because an MCP server id conflicts with another source");
            continue;
        }
        let source_uri = format!("s-code://extensions/plugins/{}", installation.bundle.id);
        for server in installation.bundle.mcp_servers {
            mcp_sources.push((
                McpServerConfig {
                    id: server.id,
                    program: server.program,
                    args: server.args,
                    environment_handles: server.environment_handles,
                    timeout_ms: server.timeout_ms,
                },
                source_uri.clone(),
            ));
        }
        for server in installation.bundle.mcp_http_servers {
            mcp_http_sources.push((
                McpHttpServerConfig {
                    id: server.id,
                    endpoint: server.endpoint,
                    header_handles: server.header_handles,
                    timeout_ms: server.timeout_ms,
                },
                source_uri.clone(),
            ));
        }
    }
    if !mcp_sources.is_empty() || !mcp_http_sources.is_empty() {
        if mcp_sources.len() + mcp_http_sources.len() > 32 {
            return Err("at most 32 MCP servers may be enabled".into());
        }
        let mut groups =
            BTreeMap::<String, (Vec<McpServerConfig>, Vec<McpHttpServerConfig>)>::new();
        for (configuration, source) in mcp_sources {
            groups.entry(source).or_default().0.push(configuration);
        }
        for (configuration, source) in mcp_http_sources {
            groups.entry(source).or_default().1.push(configuration);
        }
        let authorization: std::sync::Arc<dyn McpHttpAuthorizationProvider> = std::sync::Arc::new(
            StoreMcpOAuthAuthorizationProvider::new(store.clone(), extension_scope.clone()),
        );
        let (registry, active_mcp_sources, active_mcp_http_sources) =
            connect_mcp_source_groups(&store, &extension_scope, groups, authorization).await?;
        state = state.with_mcp_configuration_sources(registry.clone(), &active_mcp_sources);
        state = state.with_mcp_http_configuration_sources(registry, &active_mcp_http_sources);
    } else if !development_auth && config.mcp.enabled {
        warn!(
            "MCP runtime is disabled in Team Grant mode until registries and credentials are isolated per actor"
        );
    }
    let credentials = std::sync::Arc::new(EnvironmentCredentials);
    if !config.model.endpoints.is_empty() {
        let endpoints = config
            .model
            .endpoints
            .into_iter()
            .map(|endpoint| {
                let credential_handle = model_credential_handle(
                    &endpoint.provider,
                    &endpoint.base_url,
                    endpoint.credential_handle,
                );
                RoutedModelEndpoint {
                    id: endpoint.id,
                    provider_model: endpoint.provider_model,
                    provider: configured_model_provider(
                        &endpoint.provider,
                        endpoint.base_url,
                        credential_handle,
                        credentials.clone(),
                    ),
                }
            })
            .collect();
        state =
            state.with_model_provider(std::sync::Arc::new(GovernedModelRouter::new(endpoints)?));
    } else if let Some(base_url) = config.model.base_url {
        let credential_handle = model_credential_handle(
            &config.model.provider,
            &base_url,
            config.model.credential_handle,
        );
        state = state.with_model_provider(configured_model_provider(
            &config.model.provider,
            base_url,
            credential_handle,
            credentials,
        ));
    }
    if let (Some(api_base), Some(repository)) = (
        config.connectors.github_api_base,
        config.connectors.github_repository,
    ) {
        let credential_handle = config
            .connectors
            .github_credential_handle
            .unwrap_or_else(|| "GITHUB_TOKEN".into());
        state = state.with_scm_connector(std::sync::Arc::new(GitHubEnterpriseConnector::new(
            api_base.clone(),
            repository.clone(),
            credential_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
        let ci_credential_handle = config
            .connectors
            .ci_credential_handle
            .unwrap_or_else(|| "GITHUB_TOKEN".into());
        state = state.with_ci_connector(std::sync::Arc::new(GitHubActionsConnector::new(
            api_base,
            repository,
            ci_credential_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
    }
    if let (Some(api_base), Some(project_key)) = (
        config.connectors.jira_api_base,
        config.connectors.jira_project_key,
    ) {
        let credential_handle = config
            .connectors
            .jira_credential_handle
            .unwrap_or_else(|| "JIRA_TOKEN".into());
        state = state.with_work_connector(std::sync::Arc::new(JiraWorkManagementConnector::new(
            api_base,
            project_key,
            credential_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
    }
    if let (Some(api_base), Some(table), Some(credential_handle)) = (
        config.connectors.servicenow_api_base,
        config.connectors.servicenow_table,
        config.connectors.servicenow_credential_handle,
    ) {
        state =
            state.with_service_management_connector(std::sync::Arc::new(ServiceNowConnector::new(
                api_base,
                table,
                credential_handle,
                std::sync::Arc::new(EnvironmentCredentialBroker),
                connector_approval_verifier
                    .clone()
                    .expect("validated ServiceNow Team grant trust"),
            )?));
    }
    if let Some(api_base) = config.connectors.slack_api_base {
        let credential_handle = config
            .connectors
            .slack_credential_handle
            .unwrap_or_else(|| "SLACK_BOT_TOKEN".into());
        state = state.with_chat_connector(std::sync::Arc::new(SlackConnector::new(
            api_base,
            credential_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
    } else if let Some(webhook_handle) = config.connectors.teams_webhook_handle {
        state = state.with_chat_connector(std::sync::Arc::new(TeamsConnector::new(
            webhook_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
    }
    if let Some(api_base) = config.connectors.splunk_hec_base_url {
        let credential_handle = config
            .connectors
            .splunk_credential_handle
            .unwrap_or_else(|| "SPLUNK_HEC_TOKEN".into());
        state = state.with_security_event_connector(std::sync::Arc::new(SplunkHecConnector::new(
            api_base,
            credential_handle,
            std::sync::Arc::new(EnvironmentCredentialBroker),
        )?));
    }
    let _durable_worker = state.start_durable_worker(format!("daemon-{bound}"));
    let _turn_input_worker = state.start_turn_input_worker();
    let _question_auto_resolution_worker = state.start_question_auto_resolution_worker();
    let _central_audit_worker = state.start_central_audit_worker();
    let _local_connection = if development_auth {
        let instance_id = state
            .local_instance_id()
            .ok_or("local development daemon has no instance id")?
            .to_owned();
        let connection = LocalDaemonConnection::new(format!("http://{bound}"), token, instance_id);
        let guard = publish_local_daemon_connection(&connection)?;
        eprintln!("S_CODE_CONNECTION_FILE={}", guard.path().display());
        Some(guard)
    } else {
        None
    };
    let shutdown_state = state.clone();
    let (shutdown_started, shutdown_observed) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, app(state))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            let _ = shutdown_started.send(());
        })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        result = &mut server => result?,
        _ = shutdown_observed => {
            let (stopped_terminals, runtime_drained) = shutdown_state
                .shutdown_runtime_scopes(std::time::Duration::from_secs(2))
                .await;
            if stopped_terminals > 0 {
                info!(count = stopped_terminals, "stopped background terminals during daemon shutdown");
            }
            if !runtime_drained {
                warn!("forcing daemon shutdown after runtime scopes exceeded the two-second drain window");
            }
            match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
                Ok(result) => result?,
                Err(_) => warn!("forcing daemon shutdown after open clients exceeded the five-second drain window"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use s_code_audit::{CENTRAL_AUDIT_SCHEMA_VERSION, CentralAuditBatchPayload};

    #[tokio::test]
    async fn unavailable_installed_mcp_is_disabled_without_bricking_startup() {
        let store = Store::in_memory().await.unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("user".into()),
            goal_id: None,
            task_id: None,
        };
        let server = s_code_protocol::McpServerSpec {
            id: "broken-after-install".into(),
            program: "/bin/false".into(),
            args: Vec::new(),
            environment_handles: BTreeMap::new(),
            timeout_ms: 500,
        };
        store
            .install_mcp_server(&scope, &server, "approved-digest")
            .await
            .unwrap();
        let mut groups = McpSourceGroups::new();
        groups.insert(
            "s-code://extensions/mcp/broken-after-install".into(),
            (
                vec![McpServerConfig {
                    id: server.id.clone(),
                    program: server.program.clone(),
                    args: server.args.clone(),
                    environment_handles: server.environment_handles.clone(),
                    timeout_ms: server.timeout_ms,
                }],
                Vec::new(),
            ),
        );
        let authorization: std::sync::Arc<dyn McpHttpAuthorizationProvider> = std::sync::Arc::new(
            StoreMcpOAuthAuthorizationProvider::new(store.clone(), scope.clone()),
        );

        let (registry, active_stdio, active_http) =
            connect_mcp_source_groups(&store, &scope, groups, authorization)
                .await
                .unwrap();

        assert!(registry.server_ids().is_empty());
        assert!(active_stdio.is_empty());
        assert!(active_http.is_empty());
        assert!(
            !store
                .get_mcp_installation(&scope, &server.id)
                .await
                .unwrap()
                .enabled
        );
    }

    #[tokio::test]
    async fn fresh_local_database_gets_a_stable_private_managed_key() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state").join("s-code.db");
        let url = format!("sqlite://{}", database.display());
        let first = resolve_storage_connection(&url, None, None).await.unwrap();
        let StorageConnection::Encrypted {
            key: first_key,
            protection,
        } = first
        else {
            panic!("fresh database must be encrypted");
        };
        assert_eq!(protection, "managed_encrypted");
        let key_path = managed_storage_key_path(&database).unwrap();
        assert!(key_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(key_path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let second = resolve_storage_connection(&url, None, None).await.unwrap();
        let StorageConnection::Encrypted {
            key: second_key, ..
        } = second
        else {
            panic!("managed key must remain encrypted");
        };
        assert_eq!(first_key, second_key);
    }

    #[cfg(unix)]
    #[test]
    fn managed_key_creation_never_restricts_an_existing_parent_directory() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let shared_parent = directory.path().join("shared-parent");
        std::fs::create_dir(&shared_parent).unwrap();
        std::fs::set_permissions(&shared_parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let key_path = shared_parent
            .join("private-state")
            .join(".s-code.db.storage-key");

        let key = create_private_managed_key(&key_path).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(
            std::fs::metadata(&shared_parent)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(key_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_first_starts_converge_on_one_managed_key() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state").join("s-code.db");
        let url = format!("sqlite://{}", database.display());
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
        let workers = (0..8)
            .map(|_| {
                let url = url.clone();
                let barrier = barrier.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    let StorageConnection::Encrypted { key, protection } =
                        resolve_storage_connection(&url, None, None).await.unwrap()
                    else {
                        panic!("fresh database must be encrypted");
                    };
                    assert_eq!(protection, "managed_encrypted");
                    key
                })
            })
            .collect::<Vec<_>>();
        let mut keys = Vec::new();
        for worker in workers {
            keys.push(worker.await.unwrap());
        }
        assert!(keys.iter().all(|key| key == &keys[0]));
    }

    #[tokio::test]
    async fn existing_database_without_a_key_is_never_silently_rewritten() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("legacy.db");
        std::fs::write(&database, b"legacy-state").unwrap();
        let connection =
            resolve_storage_connection(&format!("sqlite://{}", database.display()), None, None)
                .await;
        assert!(connection.is_err());
        assert!(!managed_storage_key_path(&database).unwrap().exists());
    }

    #[tokio::test]
    async fn legacy_plaintext_database_remains_available_without_a_managed_key() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state").join("s-code.db");
        let url = format!("sqlite://{}", database.display());
        let store = Store::connect(&url).await.unwrap();
        store.get_or_create_device_id().await.unwrap();
        store.close().await;

        let connection = resolve_storage_connection(&url, None, None).await.unwrap();
        assert!(matches!(
            connection,
            StorageConnection::Plaintext {
                protection: "legacy_plaintext"
            }
        ));
        assert!(!managed_storage_key_path(&database).unwrap().exists());
    }

    #[tokio::test]
    async fn missing_managed_key_fails_closed_without_changing_database() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("state").join("s-code.db");
        let url = format!("sqlite://{}", database.display());
        let (store, protection) = connect_resolved_store(&url, None, None).await.unwrap();
        assert_eq!(protection, "managed_encrypted");
        store.get_or_create_device_id().await.unwrap();
        store.close().await;
        let key_path = managed_storage_key_path(&database).unwrap();
        std::fs::remove_file(&key_path).unwrap();
        let before = std::fs::read(&database).unwrap();

        let error = resolve_storage_connection(&url, None, None)
            .await
            .err()
            .unwrap();
        assert!(error.contains("managed key file is missing"));
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert!(!key_path.exists());
    }

    #[test]
    fn managed_restore_rolls_back_database_when_key_publication_fails() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("s-code.db");
        let destination_key = managed_storage_key_path(&destination).unwrap();
        std::fs::write(&destination, b"old-database").unwrap();
        replace_private_managed_key(&destination_key, &[7_u8; 32]).unwrap();
        let paths = restore_paths(&destination).unwrap();
        std::fs::write(&paths.staged_database, b"new-database").unwrap();

        let error = publish_restored_database(&destination, true).unwrap_err();
        assert!(error.contains("restored managed key"));
        assert_eq!(std::fs::read(&destination).unwrap(), b"old-database");
        assert_eq!(
            load_private_managed_key(&destination_key).unwrap(),
            [7_u8; 32]
        );
        assert!(!paths.marker.exists());
        assert!(!paths.previous_database.exists());
        assert!(!paths.previous_key.exists());
    }

    #[test]
    fn interrupted_managed_restore_is_recovered_on_next_start() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("s-code.db");
        let destination_key = managed_storage_key_path(&destination).unwrap();
        std::fs::write(&destination, b"old-database").unwrap();
        replace_private_managed_key(&destination_key, &[11_u8; 32]).unwrap();
        let paths = restore_paths(&destination).unwrap();
        std::fs::write(&paths.staged_database, b"new-database").unwrap();
        replace_private_managed_key(&paths.staged_key, &[12_u8; 32]).unwrap();
        write_private_json(
            &paths.marker,
            &RestoreJournal {
                replace_managed_key: true,
                had_previous_database: true,
                had_previous_key: true,
                phase: "publishing".into(),
            },
        )
        .unwrap();
        std::fs::rename(&destination, &paths.previous_database).unwrap();
        std::fs::rename(&destination_key, &paths.previous_key).unwrap();

        recover_interrupted_restore(&destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"old-database");
        assert_eq!(
            load_private_managed_key(&destination_key).unwrap(),
            [11_u8; 32]
        );
        assert!(!paths.marker.exists());
        assert!(!paths.staged_database.exists());
        assert!(!paths.staged_key.exists());
    }

    #[test]
    fn interrupted_first_restore_removes_unpaired_database_and_restores_bootstrap_key() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("s-code.db");
        let paths = restore_paths(&destination).unwrap();
        replace_private_managed_key(&paths.destination_key, &[21_u8; 32]).unwrap();
        std::fs::write(&paths.staged_database, b"new-database").unwrap();
        replace_private_managed_key(&paths.staged_key, &[22_u8; 32]).unwrap();
        write_private_json(
            &paths.marker,
            &RestoreJournal {
                replace_managed_key: true,
                had_previous_database: false,
                had_previous_key: true,
                phase: "publishing".into(),
            },
        )
        .unwrap();
        std::fs::rename(&paths.destination_key, &paths.previous_key).unwrap();
        std::fs::rename(&paths.staged_database, &destination).unwrap();

        recover_interrupted_restore(&destination).unwrap();
        assert!(!destination.exists());
        assert_eq!(
            load_private_managed_key(&paths.destination_key).unwrap(),
            [21_u8; 32]
        );
        assert!(!paths.marker.exists());
        assert!(!paths.staged_key.exists());
    }

    #[test]
    fn local_default_model_api_needs_no_client_credential() {
        assert_eq!(
            model_credential_handle(
                "openai_compatible",
                s_code_config::DEFAULT_MODEL_API_BASE_URL,
                None
            ),
            None
        );
        assert_eq!(
            model_credential_handle("openai_compatible", "http://127.0.0.1:54321/v1", None),
            None
        );
        assert_eq!(
            model_credential_handle("openai_compatible", "https://models.example/v1", None)
                .as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert!(model_credentials_are_available(&ModelConfig::default()));

        let remote = ModelConfig {
            base_url: Some("https://models.example/v1".into()),
            credential_handle: Some("S_CODE_TEST_MISSING_MODEL_KEY".into()),
            ..ModelConfig::default()
        };
        assert!(!model_credentials_are_available(&remote));
    }

    struct StaticCredentialBroker;

    #[async_trait::async_trait]
    impl CredentialBroker for StaticCredentialBroker {
        async fn issue(
            &self,
            handle: &str,
            _: &ActionContext,
        ) -> Result<String, s_code_connector_sdk::ConnectorError> {
            assert_eq!(handle, "AUDIT_KMS_TOKEN");
            Ok("scoped-kms-token".into())
        }
    }

    fn audit_envelope() -> SignedCentralAuditBatch {
        SignedCentralAuditBatch {
            key_id: "key".into(),
            payload: CentralAuditBatchPayload {
                schema_version: CENTRAL_AUDIT_SCHEMA_VERSION,
                batch_id: Id("batch".into()),
                source_id: Id("source".into()),
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                previous_sequence: 0,
                previous_chain_hash: "0".repeat(64),
                generated_at: chrono::Utc::now(),
                encrypted_content: None,
                records: Vec::new(),
            },
            signature: "signature".into(),
        }
    }

    #[tokio::test]
    async fn central_audit_http_delivery_accepts_bounded_receipts_and_rejects_redirects() {
        let receipt = CentralAuditIngestReceipt {
            batch_id: Id("batch".into()),
            source_id: Id("source".into()),
            accepted: true,
            record_count: 0,
            last_sequence: 0,
            last_local_sequence: 0,
            chain_head: "0".repeat(64),
            payload_sha256: "0".repeat(64),
        };
        let app = axum::Router::new()
            .route(
                "/ingest",
                axum::routing::post({
                    let receipt = receipt.clone();
                    move || {
                        let receipt = receipt.clone();
                        async move { axum::Json(receipt) }
                    }
                }),
            )
            .route(
                "/redirect",
                axum::routing::post(|| async { axum::response::Redirect::temporary("/ingest") }),
            )
            .route(
                "/oversized",
                axum::routing::post(|| async {
                    let chunks = futures_util::stream::iter([
                        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![
                            b'x';
                            40 * 1024
                        ])),
                        Ok(axum::body::Bytes::from(vec![b'y'; 40 * 1024])),
                    ]);
                    axum::response::Response::new(axum::body::Body::from_stream(chunks))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app).into_future());

        let accepted = HttpCentralAuditDelivery::new(format!("http://{address}/ingest"))
            .unwrap()
            .deliver(&audit_envelope())
            .await
            .unwrap();
        assert_eq!(accepted, receipt);
        let redirected = HttpCentralAuditDelivery::new(format!("http://{address}/redirect"))
            .unwrap()
            .deliver(&audit_envelope())
            .await
            .unwrap_err();
        assert!(redirected.to_string().contains("HTTP 307"));
        let oversized = HttpCentralAuditDelivery::new(format!("http://{address}/oversized"))
            .unwrap()
            .deliver(&audit_envelope())
            .await
            .unwrap_err();
        assert!(oversized.to_string().contains("exceeds 65536 bytes"));
        server.abort();
    }

    #[tokio::test]
    async fn central_audit_kms_contract_is_scoped_bounded_and_redirect_safe() {
        let app = axum::Router::new()
            .route(
                "/kms",
                axum::routing::post(
                    |headers: axum::http::HeaderMap,
                     axum::Json(request): axum::Json<serde_json::Value>| async move {
                        assert_eq!(
                            headers.get(axum::http::header::AUTHORIZATION).unwrap(),
                            "Bearer scoped-kms-token"
                        );
                        assert_eq!(request["organization_id"], "org");
                        assert_eq!(request["team_id"], "team");
                        assert_eq!(request["batch_id"], "batch");
                        assert_eq!(request["key_id"], "kms/org/team/audit-content");
                        assert_eq!(request["purpose"], "s-code.central-audit.content.v1");
                        axum::Json(serde_json::json!({
                            "plaintext_data_key_base64": URL_SAFE_NO_PAD.encode([73_u8; 32]),
                            "wrapped_data_key": {
                                "key_id": "kms/org/team/audit-content",
                                "algorithm": "AES-256-GCM",
                                "nonce": URL_SAFE_NO_PAD.encode([8_u8; 12]),
                                "ciphertext": URL_SAFE_NO_PAD.encode([9_u8; 48])
                            }
                        }))
                    },
                ),
            )
            .route(
                "/kms-redirect",
                axum::routing::post(|| async { axum::response::Redirect::temporary("/kms") }),
            )
            .route(
                "/kms-oversized",
                axum::routing::post(|| async {
                    let chunks = futures_util::stream::iter([
                        Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![
                            b'x';
                            40 * 1024
                        ])),
                        Ok(axum::body::Bytes::from(vec![b'y'; 40 * 1024])),
                    ]);
                    axum::response::Response::new(axum::body::Body::from_stream(chunks))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let provider = HttpCentralAuditDataKeyProvider::new(
            format!("http://{address}/kms"),
            "AUDIT_KMS_TOKEN".into(),
            std::sync::Arc::new(StaticCredentialBroker),
        )
        .unwrap();
        let material = provider
            .generate_data_key(
                &Id("org".into()),
                &Id("team".into()),
                &Id("batch".into()),
                "kms/org/team/audit-content",
            )
            .await
            .unwrap();
        assert_eq!(material.key_id(), "kms/org/team/audit-content");

        let redirected = HttpCentralAuditDataKeyProvider::new(
            format!("http://{address}/kms-redirect"),
            "AUDIT_KMS_TOKEN".into(),
            std::sync::Arc::new(StaticCredentialBroker),
        )
        .unwrap()
        .generate_data_key(
            &Id("org".into()),
            &Id("team".into()),
            &Id("batch".into()),
            "kms/org/team/audit-content",
        )
        .await
        .err()
        .expect("redirected KMS request must fail");
        assert!(redirected.to_string().contains("HTTP 307"));
        let oversized = HttpCentralAuditDataKeyProvider::new(
            format!("http://{address}/kms-oversized"),
            "AUDIT_KMS_TOKEN".into(),
            std::sync::Arc::new(StaticCredentialBroker),
        )
        .unwrap()
        .generate_data_key(
            &Id("org".into()),
            &Id("team".into()),
            &Id("batch".into()),
            "kms/org/team/audit-content",
        )
        .await
        .err()
        .expect("oversized KMS response must fail");
        assert!(oversized.to_string().contains("exceeds 65536 bytes"));
        server.abort();
    }

    fn sensitive_settings() -> DaemonSettings {
        DaemonSettings {
            organization_id: Id("org-sensitive-customer".into()),
            team_id: Id("team-secret-project".into()),
            actor_id: Id("user-alice".into()),
            workspace_uri: "file:///private/source/customer-code".into(),
            default_model: "internal-model-name".into(),
            default_title: "confidential repository migration".into(),
            telemetry_enabled: false,
            max_context_tokens: 64_000,
        }
    }

    #[tokio::test]
    async fn diagnostics_are_content_and_identifier_free() {
        let store = Store::in_memory().await.unwrap();
        store.put_settings(&sensitive_settings()).await.unwrap();
        let encoded =
            serde_json::to_string(&build_diagnostic_bundle(&store).await.unwrap()).unwrap();
        for sensitive in [
            "org-sensitive-customer",
            "team-secret-project",
            "user-alice",
            "customer-code",
            "internal-model-name",
            "confidential repository migration",
        ] {
            assert!(!encoded.contains(sensitive));
        }
        assert!(encoded.contains(r#""content_included":false"#));
        assert!(encoded.contains(r#""identifiers_included":false"#));
    }

    #[test]
    fn configuration_export_is_private_and_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("configuration.json");
        let settings = sensitive_settings();
        write_private_json(&path, &settings).unwrap();
        assert_eq!(read_configuration(&path).unwrap(), settings);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn development_repairs_an_inaccessible_default_workspace() {
        let store = Store::in_memory().await.unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let client = ClientConfig {
            workspace: Some(workspace_uri.clone()),
            model: "configured-model".into(),
            ..ClientConfig::default()
        };
        assert!(
            repair_development_settings(&store, Profile::Development, &client)
                .await
                .unwrap()
        );
        let repaired = store.get_settings().await.unwrap();
        assert_eq!(repaired.workspace_uri, workspace_uri);
        assert_eq!(repaired.default_model, "configured-model");
    }

    #[tokio::test]
    async fn production_never_falls_back_to_the_process_directory() {
        let store = Store::in_memory().await.unwrap();
        assert!(
            !repair_development_settings(&store, Profile::Production, &ClientConfig::default())
                .await
                .unwrap()
        );
        assert_eq!(
            store.get_settings().await.unwrap().workspace_uri,
            "file:///workspace"
        );
    }

    #[tokio::test]
    async fn development_upgrades_a_legacy_default_model() {
        let store = Store::in_memory().await.unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let mut settings = store.get_settings().await.unwrap();
        settings.workspace_uri = workspace_uri;
        settings.default_model = "openai/gpt-oss-20b".into();
        store.put_settings(&settings).await.unwrap();

        assert!(
            repair_development_settings(&store, Profile::Development, &ClientConfig::default())
                .await
                .unwrap()
        );
        assert_eq!(
            store.get_settings().await.unwrap().default_model,
            "deepseek/deepseek-v4-flash"
        );
    }

    #[tokio::test]
    async fn development_preserves_an_explicit_non_legacy_model() {
        let store = Store::in_memory().await.unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let mut settings = store.get_settings().await.unwrap();
        settings.workspace_uri = workspace_uri;
        settings.default_model = "organization/approved-model".into();
        store.put_settings(&settings).await.unwrap();

        assert!(
            !repair_development_settings(&store, Profile::Development, &ClientConfig::default())
                .await
                .unwrap()
        );
        assert_eq!(
            store.get_settings().await.unwrap().default_model,
            "organization/approved-model"
        );
    }
}
