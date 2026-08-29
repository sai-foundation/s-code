//! Versioned, provenance-aware configuration for every OpenCoding process.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_MODEL_API_BASE_URL: &str = "http://127.0.0.1:18787/v1";
pub const LOCAL_DAEMON_CONNECTION_SCHEMA_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_CONNECTION_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDaemonConnection {
    pub schema_version: u32,
    pub daemon_url: String,
    pub token: String,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
}

impl LocalDaemonConnection {
    pub fn new(daemon_url: String, token: String) -> Self {
        Self {
            schema_version: LOCAL_DAEMON_CONNECTION_SCHEMA_VERSION,
            daemon_url,
            token,
            pid: std::process::id(),
            started_at: Utc::now(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != LOCAL_DAEMON_CONNECTION_SCHEMA_VERSION {
            return Err("unsupported local daemon connection schema".into());
        }
        let url = url::Url::parse(&self.daemon_url)
            .map_err(|_| "local daemon connection has an invalid URL")?;
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if url.scheme() != "http" || !loopback || url.path() != "/" {
            return Err("local daemon connection URL must be a loopback HTTP origin".into());
        }
        if self.token.is_empty()
            || self.token.len() > 4096
            || self.token.chars().any(char::is_whitespace)
            || self.pid == 0
        {
            return Err("local daemon connection credential is invalid".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct LocalDaemonConnectionGuard {
    path: PathBuf,
    pid: u32,
}

impl LocalDaemonConnectionGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LocalDaemonConnectionGuard {
    fn drop(&mut self) {
        let belongs_to_process = read_local_daemon_connection_from(&self.path)
            .is_ok_and(|connection| connection.pid == self.pid);
        if belongs_to_process {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn local_daemon_connection_path() -> Result<PathBuf, String> {
    if let Some(directory) = std::env::var_os("OPENCODING_RUNTIME_DIR") {
        if directory.is_empty() {
            return Err("OPENCODING_RUNTIME_DIR must not be empty".into());
        }
        return Ok(PathBuf::from(directory).join("daemon.json"));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    Ok(PathBuf::from(home)
        .join(".opencoding")
        .join("run")
        .join("daemon.json"))
}

pub fn read_local_daemon_connection() -> Result<LocalDaemonConnection, String> {
    read_local_daemon_connection_from(&local_daemon_connection_path()?)
}

fn read_local_daemon_connection_from(path: &Path) -> Result<LocalDaemonConnection, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("local daemon connection must be a regular file".into());
    }
    if metadata.len() > MAX_CONNECTION_BYTES {
        return Err("local daemon connection file is too large".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("local daemon connection file must use mode 0600".into());
        }
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let connection: LocalDaemonConnection =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    connection.validate()?;
    Ok(connection)
}

pub fn publish_local_daemon_connection(
    connection: &LocalDaemonConnection,
) -> Result<LocalDaemonConnectionGuard, String> {
    publish_local_daemon_connection_to(connection, local_daemon_connection_path()?)
}

fn publish_local_daemon_connection_to(
    connection: &LocalDaemonConnection,
    path: PathBuf,
) -> Result<LocalDaemonConnectionGuard, String> {
    connection.validate()?;
    if connection.pid != std::process::id() {
        return Err("local daemon connection PID does not match this process".into());
    }
    let directory = path
        .parent()
        .ok_or("local daemon connection path has no parent")?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let directory_metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err("local daemon runtime directory must be a regular directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("cannot secure {}: {error}", directory.display()))?;
    }
    let temporary = directory.join(format!(
        ".daemon.json.{}.{}.tmp",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<(), String> {
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        let bytes = serde_json::to_vec(connection).map_err(|error| error.to_string())?;
        file.write_all(&bytes)
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .map_err(|error| format!("cannot publish {}: {error}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(LocalDaemonConnectionGuard {
        path,
        pid: connection.pid,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Component {
    Daemon,
    ControlPlane,
    Runner,
    Cli,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Development,
    Production,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Default,
    File,
    Environment,
    SecretEnvironment,
    Cli,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadBehavior {
    /// A process restart is required. This is the safe default.
    RestartRequired,
    /// This field belongs to the separately signed Team runtime configuration.
    SignedTeamRuntime,
}

pub fn reload_behavior(path: &str) -> ReloadBehavior {
    if path.starts_with("team_runtime.") {
        ReloadBehavior::SignedTeamRuntime
    } else {
        ReloadBehavior::RestartRequired
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Provenance {
    pub source: SourceKind,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct EffectiveConfig {
    pub config: RootConfig,
    provenance: BTreeMap<String, Provenance>,
}

impl EffectiveConfig {
    pub fn provenance(&self, path: &str) -> Option<&Provenance> {
        self.provenance.get(path)
    }

    pub fn provenance_map(&self) -> &BTreeMap<String, Provenance> {
        &self.provenance
    }

    pub fn redacted_json(&self) -> Value {
        let mut value = serde_json::to_value(&self.config).expect("configuration serializes");
        for path in SENSITIVE_PATHS {
            if get_path(&value, path).is_some_and(|value| !value.is_null()) {
                set_path(&mut value, path, Value::String("[REDACTED]".into()));
            }
        }
        value
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RootConfig {
    pub schema_version: u32,
    pub profile: Profile,
    /// Explicit environment-variable allowlist for secret material. Keys are
    /// configuration field paths; values are environment variable names.
    pub secret_references: BTreeMap<String, String>,
    pub daemon: DaemonConfig,
    pub control_plane: ControlPlaneConfig,
    pub runner: RunnerProcessConfig,
    pub client: ClientConfig,
    pub model: ModelConfig,
    pub connectors: ConnectorConfig,
    pub mcp: McpConfig,
}

impl Default for RootConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            profile: Profile::Development,
            secret_references: BTreeMap::new(),
            daemon: DaemonConfig::default(),
            control_plane: ControlPlaneConfig::default(),
            runner: RunnerProcessConfig::default(),
            client: ClientConfig::default(),
            model: ModelConfig::default(),
            connectors: ConnectorConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub listen: String,
    pub database_url: String,
    pub auth_mode: String,
    pub token: Option<String>,
    pub runner_token: Option<String>,
    pub team_grant_issuer: String,
    pub team_grant_audience: String,
    pub team_grant_key_id: Option<String>,
    pub team_grant_public_key_base64: Option<String>,
    pub storage_encryption_key_id: Option<String>,
    pub storage_encryption_key_base64: Option<String>,
    pub team_config_key_id: Option<String>,
    pub team_config_public_key_base64: Option<String>,
    pub central_audit: CentralAuditConfig,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:0".into(),
            database_url: "sqlite://opencoding.db".into(),
            auth_mode: "development_token".into(),
            token: None,
            runner_token: None,
            team_grant_issuer: "opencoding-control-plane".into(),
            team_grant_audience: "opencoding-daemon".into(),
            team_grant_key_id: None,
            team_grant_public_key_base64: None,
            storage_encryption_key_id: None,
            storage_encryption_key_base64: None,
            team_config_key_id: None,
            team_config_public_key_base64: None,
            central_audit: CentralAuditConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CentralAuditConfig {
    pub enabled: bool,
    pub ingest_url: Option<String>,
    pub source_id: Option<String>,
    pub organization_id: Option<String>,
    pub team_id: Option<String>,
    pub key_id: Option<String>,
    pub private_key_base64: Option<String>,
    pub kms_generate_data_key_url: Option<String>,
    pub kms_credential_handle: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlPlaneConfig {
    pub listen: String,
    pub database_url: String,
    pub database_provider: String,
    pub nats_url: String,
    pub nats_stream: String,
    pub nats_replicas: usize,
    pub nats_ca_file: Option<String>,
    pub oidc_ca_file: Option<String>,
    pub oidc_issuer: String,
    pub oidc_audience: String,
    pub oidc_jwks_uri: String,
    pub oidc_authorization_endpoint: String,
    pub oidc_token_endpoint: String,
    pub oidc_client_id: String,
    pub oidc_client_secret: Option<String>,
    pub oidc_redirect_uri: String,
    pub identity_mode: String,
    pub saml_upstream_issuer: Option<String>,
    pub secure_cookie: bool,
    pub organization_id: String,
    pub organization_name: String,
    pub team_id: String,
    pub team_name: String,
    pub admin_principal_id: String,
    pub bootstrap_device_id: String,
    pub policy_key_id: String,
    pub policy_public_key_base64: String,
    pub extension_scanner_key_id: String,
    pub extension_scanner_public_key_base64: String,
    pub team_grant_key_id: Option<String>,
    pub team_grant_private_key_base64: Option<String>,
    pub team_grant_issuer: String,
    pub team_grant_audience: String,
    pub entitlement_key_id: Option<String>,
    pub entitlement_public_key_base64: Option<String>,
    pub entitlement_trust_roots: Vec<EntitlementTrustRootConfig>,
    pub entitlement_issuer: String,
    pub entitlement_token: Option<String>,
    pub audit_kms_decrypt_url: Option<String>,
    pub audit_kms_destroy_url: Option<String>,
    pub audit_kms_credential_handle: Option<String>,
}

impl Default for ControlPlaneConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            database_url: String::new(),
            database_provider: "postgresql".into(),
            nats_url: String::new(),
            nats_stream: "OPENCODING".into(),
            nats_replicas: 3,
            nats_ca_file: None,
            oidc_ca_file: None,
            oidc_issuer: String::new(),
            oidc_audience: String::new(),
            oidc_jwks_uri: String::new(),
            oidc_authorization_endpoint: String::new(),
            oidc_token_endpoint: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret: None,
            oidc_redirect_uri: String::new(),
            identity_mode: "oidc".into(),
            saml_upstream_issuer: None,
            secure_cookie: true,
            organization_id: String::new(),
            organization_name: String::new(),
            team_id: String::new(),
            team_name: String::new(),
            admin_principal_id: String::new(),
            bootstrap_device_id: String::new(),
            policy_key_id: String::new(),
            policy_public_key_base64: String::new(),
            extension_scanner_key_id: String::new(),
            extension_scanner_public_key_base64: String::new(),
            team_grant_key_id: None,
            team_grant_private_key_base64: None,
            team_grant_issuer: "opencoding-control-plane".into(),
            team_grant_audience: "opencoding-daemon".into(),
            entitlement_key_id: None,
            entitlement_public_key_base64: None,
            entitlement_trust_roots: Vec::new(),
            entitlement_issuer: "opencoding-licensing".into(),
            entitlement_token: None,
            audit_kms_decrypt_url: None,
            audit_kms_destroy_url: None,
            audit_kms_credential_handle: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementTrustRootStatusConfig {
    Active,
    Retiring,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntitlementTrustRootConfig {
    pub key_id: String,
    pub public_key_base64: String,
    pub generation: u64,
    pub status: EntitlementTrustRootStatusConfig,
    pub accept_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerProcessConfig {
    pub daemon_url: String,
    pub token: Option<String>,
    pub worker_id: Option<String>,
    pub allow_network: bool,
    pub poll_interval_millis: u64,
    pub lease_seconds: u64,
}

impl Default for RunnerProcessConfig {
    fn default() -> Self {
        Self {
            daemon_url: "http://127.0.0.1:4096".into(),
            token: None,
            worker_id: None,
            allow_network: false,
            poll_interval_millis: 500,
            lease_seconds: 30,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    pub daemon_url: String,
    pub token: Option<String>,
    pub organization_id: String,
    pub team_id: String,
    pub actor_id: String,
    pub workspace: Option<String>,
    pub model: String,
    /// Interactive CLI color palette: `dark`, `light`, or `no_color`.
    pub theme: String,
    /// Interactive CLI editing preset: `emacs` or `vim`.
    pub keymap: String,
    /// Interactive CLI footer: `full`, `compact`, or `off`.
    pub statusline: String,
    /// Optional external editor command. Arguments are parsed without a shell.
    pub editor: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            daemon_url: "http://127.0.0.1:4096".into(),
            token: None,
            organization_id: "org_local".into(),
            team_id: "team_local".into(),
            actor_id: "user_local".into(),
            workspace: None,
            model: "deepseek/deepseek-v4-flash".into(),
            theme: "dark".into(),
            keymap: "emacs".into(),
            statusline: "full".into(),
            editor: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    pub base_url: Option<String>,
    pub credential_handle: Option<String>,
    pub endpoints: Vec<ModelEndpointConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEndpointConfig {
    pub id: String,
    pub provider: String,
    pub provider_model: String,
    pub base_url: String,
    pub credential_handle: Option<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "openai_compatible".into(),
            base_url: Some(DEFAULT_MODEL_API_BASE_URL.into()),
            credential_handle: None,
            endpoints: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectorConfig {
    pub github_api_base: Option<String>,
    pub github_repository: Option<String>,
    pub github_credential_handle: Option<String>,
    pub ci_credential_handle: Option<String>,
    pub jira_api_base: Option<String>,
    pub jira_project_key: Option<String>,
    pub jira_credential_handle: Option<String>,
    pub servicenow_api_base: Option<String>,
    pub servicenow_table: Option<String>,
    pub servicenow_credential_handle: Option<String>,
    pub slack_api_base: Option<String>,
    pub slack_credential_handle: Option<String>,
    pub teams_webhook_handle: Option<String>,
    pub splunk_hec_base_url: Option<String>,
    pub splunk_credential_handle: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpConfig {
    pub enabled: bool,
    pub config_path: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read configuration {path}: {message}")]
    Read { path: PathBuf, message: String },
    #[error("configuration {0} exceeds 1 MiB")]
    TooLarge(PathBuf),
    #[error("invalid configuration: {0}")]
    Invalid(String),
    #[error("unsupported configuration schema version {0}")]
    UnsupportedVersion(u32),
}

#[derive(Default)]
pub struct ConfigLoader {
    file: Option<PathBuf>,
    environment: BTreeMap<String, String>,
    overrides: Vec<(String, Value, String)>,
}

impl ConfigLoader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_process() -> Self {
        let environment = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect::<BTreeMap<_, _>>();
        let file = environment.get("OPENCODING_CONFIG").map(PathBuf::from);
        Self {
            file,
            environment,
            overrides: Vec::new(),
        }
    }

    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    pub fn with_environment<I, K, V>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.environment = values
            .into_iter()
            .filter_map(|(key, value)| {
                Some((
                    key.into().into_string().ok()?,
                    value.into().into_string().ok()?,
                ))
            })
            .collect();
        self
    }

    pub fn with_cli_override(mut self, path: impl Into<String>, value: Value) -> Self {
        let path = path.into();
        self.overrides
            .push((path.clone(), value, format!("--set {path}")));
        self
    }

    pub fn load(self, component: Component) -> Result<EffectiveConfig, ConfigError> {
        let mut value = serde_json::to_value(RootConfig::default()).expect("defaults serialize");
        let mut provenance = BTreeMap::new();
        record_leaves(
            &value,
            "",
            SourceKind::Default,
            "compiled default",
            &mut provenance,
        );

        let mut file_metadata = None;
        if let Some(path) = &self.file {
            let link_metadata =
                std::fs::symlink_metadata(path).map_err(|error| ConfigError::Read {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
            if link_metadata.file_type().is_symlink() {
                return Err(ConfigError::Invalid(format!(
                    "configuration {} must not be a symbolic link",
                    path.display()
                )));
            }
            let metadata = std::fs::metadata(path).map_err(|error| ConfigError::Read {
                path: path.clone(),
                message: error.to_string(),
            })?;
            if metadata.len() > MAX_CONFIG_BYTES {
                return Err(ConfigError::TooLarge(path.clone()));
            }
            let bytes = std::fs::read(path).map_err(|error| ConfigError::Read {
                path: path.clone(),
                message: error.to_string(),
            })?;
            let file_value: Value = if path.extension().and_then(|value| value.to_str())
                == Some("json")
            {
                serde_json::from_slice(&bytes)
                    .map_err(|error| ConfigError::Invalid(format!("{}: {error}", path.display())))?
            } else {
                let text = std::str::from_utf8(&bytes).map_err(|error| {
                    ConfigError::Invalid(format!("{}: {error}", path.display()))
                })?;
                let value: toml::Value = toml::from_str(text).map_err(|error| {
                    ConfigError::Invalid(format!("{}: {error}", path.display()))
                })?;
                serde_json::to_value(value).expect("TOML value serializes")
            };
            reject_exposed_secret_file(path, &metadata, &file_value)?;
            merge(
                &mut value,
                file_value,
                "",
                SourceKind::File,
                &path.display().to_string(),
                &mut provenance,
            )?;
            file_metadata = Some(metadata);
        }

        let profile = serde_json::from_value::<Profile>(
            value
                .get("profile")
                .cloned()
                .unwrap_or(Value::String("development".into())),
        )
        .map_err(|error| ConfigError::Invalid(format!("profile: {error}")))?;
        if profile == Profile::Production {
            let Some(metadata) = file_metadata.as_ref() else {
                return Err(ConfigError::Invalid(
                    "production profile requires a configuration file".into(),
                ));
            };
            reject_mutable_production_file(self.file.as_deref().expect("file checked"), metadata)?;
            if !self.overrides.is_empty() {
                return Err(ConfigError::Invalid(
                    "CLI field overrides are disabled in production".into(),
                ));
            }
        } else {
            for mapping in ENV_MAPPINGS {
                if let Some(raw) = self.environment.get(mapping.env) {
                    let parsed = parse_env(raw, mapping.kind).map_err(|message| {
                        ConfigError::Invalid(format!("{}: {message}", mapping.env))
                    })?;
                    set_path(&mut value, mapping.path, parsed);
                    provenance.insert(
                        mapping.path.into(),
                        Provenance {
                            source: SourceKind::Environment,
                            detail: mapping.env.into(),
                        },
                    );
                }
            }
        }
        apply_secret_references(&mut value, &self.environment, &mut provenance)?;
        for (path, override_value, detail) in self.overrides {
            set_path(&mut value, &path, override_value);
            provenance.insert(
                path,
                Provenance {
                    source: SourceKind::Cli,
                    detail,
                },
            );
        }
        let mut config: RootConfig = serde_json::from_value(value)
            .map_err(|error| ConfigError::Invalid(error.to_string()))?;
        if !config.model.endpoints.is_empty()
            && provenance
                .get("model.base_url")
                .is_some_and(|entry| entry.source == SourceKind::Default)
        {
            config.model.base_url = None;
        }
        validate(&config, component)?;
        if config.profile == Profile::Production {
            validate_production(&config, component, &provenance)?;
        }
        Ok(EffectiveConfig { config, provenance })
    }
}

#[derive(Clone, Copy)]
enum EnvKind {
    String,
    Bool,
    U64,
}
struct EnvMapping {
    env: &'static str,
    path: &'static str,
    kind: EnvKind,
}
const ENV_MAPPINGS: &[EnvMapping] = &[
    EnvMapping {
        env: "OPENCODING_DAEMON_LISTEN",
        path: "daemon.listen",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_DATABASE_URL",
        path: "daemon.database_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_DAEMON_AUTH_MODE",
        path: "daemon.auth_mode",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TOKEN",
        path: "daemon.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_TOKEN",
        path: "daemon.runner_token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAM_GRANT_KEY_ID",
        path: "daemon.team_grant_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAM_GRANT_PUBLIC_KEY_BASE64",
        path: "daemon.team_grant_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_STORAGE_ENCRYPTION_KEY_ID",
        path: "daemon.storage_encryption_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_STORAGE_ENCRYPTION_KEY_BASE64",
        path: "daemon.storage_encryption_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAM_CONFIG_KEY_ID",
        path: "daemon.team_config_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAM_CONFIG_PUBLIC_KEY_BASE64",
        path: "daemon.team_config_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_MODEL_PROVIDER",
        path: "model.provider",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_MODEL_BASE_URL",
        path: "model.base_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_MODEL_CREDENTIAL_HANDLE",
        path: "model.credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_MCP_ENABLED",
        path: "mcp.enabled",
        kind: EnvKind::Bool,
    },
    EnvMapping {
        env: "OPENCODING_MCP_CONFIG",
        path: "mcp.config_path",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_DAEMON_URL",
        path: "runner.daemon_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_TOKEN",
        path: "runner.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_ID",
        path: "runner.worker_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_ALLOW_NETWORK",
        path: "runner.allow_network",
        kind: EnvKind::Bool,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_POLL_INTERVAL_MILLIS",
        path: "runner.poll_interval_millis",
        kind: EnvKind::U64,
    },
    EnvMapping {
        env: "OPENCODING_RUNNER_LEASE_SECONDS",
        path: "runner.lease_seconds",
        kind: EnvKind::U64,
    },
    EnvMapping {
        env: "OPENCODING_URL",
        path: "client.daemon_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TOKEN",
        path: "client.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ORGANIZATION",
        path: "client.organization_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAM",
        path: "client.team_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ACTOR",
        path: "client.actor_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_WORKSPACE",
        path: "client.workspace",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_MODEL",
        path: "client.model",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_THEME",
        path: "client.theme",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_KEYMAP",
        path: "client.keymap",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_STATUSLINE",
        path: "client.statusline",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_EDITOR",
        path: "client.editor",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_LISTEN",
        path: "control_plane.listen",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_DATABASE_URL",
        path: "control_plane.database_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_DATABASE_PROVIDER",
        path: "control_plane.database_provider",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_NATS_URL",
        path: "control_plane.nats_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_NATS_STREAM",
        path: "control_plane.nats_stream",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_NATS_REPLICAS",
        path: "control_plane.nats_replicas",
        kind: EnvKind::U64,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_ISSUER",
        path: "control_plane.oidc_issuer",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_AUDIENCE",
        path: "control_plane.oidc_audience",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_JWKS_URI",
        path: "control_plane.oidc_jwks_uri",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_AUTHORIZATION_ENDPOINT",
        path: "control_plane.oidc_authorization_endpoint",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_TOKEN_ENDPOINT",
        path: "control_plane.oidc_token_endpoint",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_CLIENT_ID",
        path: "control_plane.oidc_client_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_CLIENT_SECRET",
        path: "control_plane.oidc_client_secret",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_OIDC_REDIRECT_URI",
        path: "control_plane.oidc_redirect_uri",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_IDENTITY_MODE",
        path: "control_plane.identity_mode",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SAML_UPSTREAM_ISSUER",
        path: "control_plane.saml_upstream_issuer",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BROWSER_COOKIE_SECURE",
        path: "control_plane.secure_cookie",
        kind: EnvKind::Bool,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_ORGANIZATION_ID",
        path: "control_plane.organization_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_ORGANIZATION_NAME",
        path: "control_plane.organization_name",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_TEAM_ID",
        path: "control_plane.team_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_TEAM_NAME",
        path: "control_plane.team_name",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_ADMIN_PRINCIPAL_ID",
        path: "control_plane.admin_principal_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_BOOTSTRAP_DEVICE_ID",
        path: "control_plane.bootstrap_device_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_POLICY_KEY_ID",
        path: "control_plane.policy_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_POLICY_PUBLIC_KEY_BASE64",
        path: "control_plane.policy_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_EXTENSION_SCANNER_KEY_ID",
        path: "control_plane.extension_scanner_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_EXTENSION_SCANNER_PUBLIC_KEY_BASE64",
        path: "control_plane.extension_scanner_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_TEAM_GRANT_KEY_ID",
        path: "control_plane.team_grant_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_TEAM_GRANT_PRIVATE_KEY_BASE64",
        path: "control_plane.team_grant_private_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_TEAM_GRANT_ISSUER",
        path: "control_plane.team_grant_issuer",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_TEAM_GRANT_AUDIENCE",
        path: "control_plane.team_grant_audience",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ENTITLEMENT_KEY_ID",
        path: "control_plane.entitlement_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ENTITLEMENT_PUBLIC_KEY_BASE64",
        path: "control_plane.entitlement_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ENTITLEMENT_ISSUER",
        path: "control_plane.entitlement_issuer",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_ENTITLEMENT_TOKEN",
        path: "control_plane.entitlement_token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_AUDIT_KMS_DECRYPT_URL",
        path: "control_plane.audit_kms_decrypt_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_AUDIT_KMS_DESTROY_URL",
        path: "control_plane.audit_kms_destroy_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CONTROL_AUDIT_KMS_CREDENTIAL_HANDLE",
        path: "control_plane.audit_kms_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_GITHUB_API_BASE",
        path: "connectors.github_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_GITHUB_REPOSITORY",
        path: "connectors.github_repository",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_GITHUB_CREDENTIAL_HANDLE",
        path: "connectors.github_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_CI_CREDENTIAL_HANDLE",
        path: "connectors.ci_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_JIRA_API_BASE",
        path: "connectors.jira_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_JIRA_PROJECT_KEY",
        path: "connectors.jira_project_key",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_JIRA_CREDENTIAL_HANDLE",
        path: "connectors.jira_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SERVICENOW_API_BASE",
        path: "connectors.servicenow_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SERVICENOW_TABLE",
        path: "connectors.servicenow_table",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SERVICENOW_CREDENTIAL_HANDLE",
        path: "connectors.servicenow_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SLACK_API_BASE",
        path: "connectors.slack_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SLACK_CREDENTIAL_HANDLE",
        path: "connectors.slack_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_TEAMS_WEBHOOK_HANDLE",
        path: "connectors.teams_webhook_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SPLUNK_HEC_BASE_URL",
        path: "connectors.splunk_hec_base_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "OPENCODING_SPLUNK_CREDENTIAL_HANDLE",
        path: "connectors.splunk_credential_handle",
        kind: EnvKind::String,
    },
];

const SENSITIVE_PATHS: &[&str] = &[
    "daemon.database_url",
    "daemon.token",
    "daemon.runner_token",
    "daemon.storage_encryption_key_base64",
    "daemon.central_audit.private_key_base64",
    "control_plane.database_url",
    "control_plane.oidc_client_secret",
    "control_plane.team_grant_private_key_base64",
    "control_plane.entitlement_token",
    "runner.token",
    "client.token",
];

fn parse_env(value: &str, kind: EnvKind) -> Result<Value, String> {
    match kind {
        EnvKind::String => Ok(Value::String(value.into())),
        EnvKind::Bool => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Value::Bool(true)),
            "0" | "false" | "no" | "off" => Ok(Value::Bool(false)),
            _ => Err("expected boolean (true/false or 1/0)".into()),
        },
        EnvKind::U64 => value
            .parse::<u64>()
            .map(Value::from)
            .map_err(|_| "expected a non-negative integer".into()),
    }
}

fn validate_entitlement_trust_roots(
    roots: &[EntitlementTrustRootConfig],
) -> Result<(), ConfigError> {
    if roots.len() > 8 {
        return Err(ConfigError::Invalid(
            "at most eight entitlement trust roots are allowed".into(),
        ));
    }
    let mut key_ids = BTreeSet::new();
    let mut generations = BTreeSet::new();
    let mut active_generation = None;
    let mut retiring_generation = 0_u64;
    for root in roots {
        if root.key_id.trim().is_empty()
            || root.public_key_base64.trim().is_empty()
            || root.public_key_base64.len() > 1024
            || root.generation == 0
            || !key_ids.insert(root.key_id.as_str())
            || !generations.insert(root.generation)
        {
            return Err(ConfigError::Invalid(
                "entitlement trust roots require unique bounded keys and positive unique generations"
                    .into(),
            ));
        }
        match root.status {
            EntitlementTrustRootStatusConfig::Active => {
                if root.accept_until.is_some()
                    || active_generation.replace(root.generation).is_some()
                {
                    return Err(ConfigError::Invalid(
                        "exactly one active entitlement root without accept_until is required"
                            .into(),
                    ));
                }
            }
            EntitlementTrustRootStatusConfig::Retiring => {
                if root.accept_until.is_none() {
                    return Err(ConfigError::Invalid(
                        "retiring entitlement roots require accept_until".into(),
                    ));
                }
                retiring_generation = retiring_generation.max(root.generation);
            }
        }
    }
    if !roots.is_empty()
        && active_generation.is_none_or(|generation| generation <= retiring_generation)
    {
        return Err(ConfigError::Invalid(
            "one active entitlement root must have a newer generation than all retiring roots"
                .into(),
        ));
    }
    Ok(())
}

fn validate(config: &RootConfig, component: Component) -> Result<(), ConfigError> {
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedVersion(config.schema_version));
    }
    match component {
        Component::Daemon => {
            config.daemon.listen.parse::<SocketAddr>().map_err(|_| {
                ConfigError::Invalid("daemon.listen must be a socket address".into())
            })?;
            if config.daemon.database_url.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "daemon.database_url is required".into(),
                ));
            }
            if config.daemon.auth_mode != "development_token"
                && config.daemon.auth_mode != "team_grant"
            {
                return Err(ConfigError::Invalid(
                    "daemon.auth_mode must be development_token or team_grant".into(),
                ));
            }
            paired(
                &config.daemon.team_grant_key_id,
                &config.daemon.team_grant_public_key_base64,
                "daemon Team grant trust key",
            )?;
            paired(
                &config.daemon.storage_encryption_key_id,
                &config.daemon.storage_encryption_key_base64,
                "daemon storage encryption key",
            )?;
            if config.daemon.auth_mode == "team_grant" {
                required("daemon.team_grant_issuer", &config.daemon.team_grant_issuer)?;
                required(
                    "daemon.team_grant_audience",
                    &config.daemon.team_grant_audience,
                )?;
                required_option(&config.daemon.team_grant_key_id, "daemon.team_grant_key_id")?;
                required_option(
                    &config.daemon.team_grant_public_key_base64,
                    "daemon.team_grant_public_key_base64",
                )?;
            }
            paired(
                &config.daemon.team_config_key_id,
                &config.daemon.team_config_public_key_base64,
                "daemon Team configuration trust key",
            )?;
            let central_audit = &config.daemon.central_audit;
            let central_values_present = central_audit.ingest_url.is_some()
                || central_audit.source_id.is_some()
                || central_audit.organization_id.is_some()
                || central_audit.team_id.is_some()
                || central_audit.key_id.is_some()
                || central_audit.private_key_base64.is_some()
                || central_audit.kms_generate_data_key_url.is_some()
                || central_audit.kms_credential_handle.is_some();
            if central_audit.enabled {
                let ingest_url = central_audit.ingest_url.as_deref().ok_or_else(|| {
                    ConfigError::Invalid(
                        "daemon.central_audit.ingest_url is required when enabled".into(),
                    )
                })?;
                secure_or_loopback_url(ingest_url, "daemon.central_audit.ingest_url")?;
                for (path, value) in [
                    ("source_id", &central_audit.source_id),
                    ("organization_id", &central_audit.organization_id),
                    ("team_id", &central_audit.team_id),
                    ("key_id", &central_audit.key_id),
                    ("private_key_base64", &central_audit.private_key_base64),
                ] {
                    required_option(value, &format!("daemon.central_audit.{path}"))?;
                }
                paired(
                    &central_audit.kms_generate_data_key_url,
                    &central_audit.kms_credential_handle,
                    "daemon central audit KMS GenerateDataKey provider",
                )?;
                if let Some(url) = central_audit.kms_generate_data_key_url.as_deref() {
                    secure_or_loopback_url(url, "daemon.central_audit.kms_generate_data_key_url")?;
                    let handle = central_audit
                        .kms_credential_handle
                        .as_deref()
                        .expect("paired above");
                    if !valid_environment_name(handle) {
                        return Err(ConfigError::Invalid(
                            "daemon.central_audit.kms_credential_handle must be an environment credential name"
                                .into(),
                        ));
                    }
                }
                for (path, value) in [
                    ("source_id", &central_audit.source_id),
                    ("organization_id", &central_audit.organization_id),
                    ("team_id", &central_audit.team_id),
                    ("key_id", &central_audit.key_id),
                ] {
                    let value = value.as_deref().expect("required above");
                    if value.len() > 128
                        || value.is_empty()
                        || value.chars().any(char::is_whitespace)
                    {
                        return Err(ConfigError::Invalid(format!(
                            "daemon.central_audit.{path} must be a bounded non-whitespace identifier"
                        )));
                    }
                }
            } else if central_values_present {
                return Err(ConfigError::Invalid(
                    "daemon.central_audit fields require enabled = true".into(),
                ));
            }
            if !matches!(
                config.model.provider.as_str(),
                "openai_compatible" | "anthropic" | "gemini"
            ) {
                return Err(ConfigError::Invalid(
                    "model.provider must be openai_compatible, anthropic or gemini".into(),
                ));
            }
            if config.model.provider != "openai_compatible"
                && config.model.base_url.as_deref() == Some(DEFAULT_MODEL_API_BASE_URL)
            {
                return Err(ConfigError::Invalid(
                    "anthropic and gemini model providers require an explicit model.base_url"
                        .into(),
                ));
            }
            if !config.model.endpoints.is_empty()
                && (config.model.base_url.is_some() || config.model.credential_handle.is_some())
            {
                return Err(ConfigError::Invalid(
                    "model.endpoints cannot be combined with legacy model.base_url or model.credential_handle".into(),
                ));
            }
            let mut endpoint_ids = std::collections::BTreeSet::new();
            if config.model.endpoints.len() > 64
                || config.model.endpoints.iter().any(|endpoint| {
                    !matches!(
                        endpoint.provider.as_str(),
                        "openai_compatible" | "anthropic" | "gemini"
                    ) || endpoint.id.is_empty()
                        || endpoint.id.len() > 128
                        || !endpoint.id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric()
                                || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
                        })
                        || endpoint.provider_model.trim().is_empty()
                        || endpoint.provider_model.len() > 256
                        || endpoint.base_url.trim().is_empty()
                        || !endpoint_ids.insert(endpoint.id.as_str())
                })
            {
                return Err(ConfigError::Invalid(
                    "model.endpoints contains an invalid or duplicate endpoint".into(),
                ));
            }
            if config.model.base_url.is_none() && config.model.credential_handle.is_some() {
                return Err(ConfigError::Invalid(
                    "model.credential_handle requires model.base_url".into(),
                ));
            }
            let servicenow = [
                config.connectors.servicenow_api_base.is_some(),
                config.connectors.servicenow_table.is_some(),
                config.connectors.servicenow_credential_handle.is_some(),
            ];
            if servicenow.iter().any(|value| *value) && !servicenow.iter().all(|value| *value) {
                return Err(ConfigError::Invalid(
                    "ServiceNow API base, table and credential handle must be configured together"
                        .into(),
                ));
            }
            if servicenow.iter().all(|value| *value) && config.daemon.auth_mode != "team_grant" {
                return Err(ConfigError::Invalid(
                    "ServiceNow requires signed Team grant authentication for four-eyes approval verification"
                        .into(),
                ));
            }
            if config.mcp.enabled && config.mcp.config_path.is_none() {
                return Err(ConfigError::Invalid(
                    "mcp.config_path is required when MCP is enabled".into(),
                ));
            }
        }
        Component::Runner => {
            secure_or_loopback_url(&config.runner.daemon_url, "runner.daemon_url")?;
            required_option(&config.runner.token, "runner.token")?;
            if !(15..=300).contains(&config.runner.lease_seconds) {
                return Err(ConfigError::Invalid(
                    "runner.lease_seconds must be between 15 and 300".into(),
                ));
            }
            if config.runner.poll_interval_millis < 50 {
                return Err(ConfigError::Invalid(
                    "runner.poll_interval_millis must be at least 50".into(),
                ));
            }
        }
        Component::Cli => {
            secure_or_loopback_url(&config.client.daemon_url, "client.daemon_url")?;
            if config.profile == Profile::Production {
                required_option(&config.client.token, "client.token")?;
            }
            for (name, value) in [
                ("client.organization_id", &config.client.organization_id),
                ("client.team_id", &config.client.team_id),
                ("client.actor_id", &config.client.actor_id),
                ("client.model", &config.client.model),
            ] {
                required(name, value)?;
            }
            if !matches!(config.client.theme.as_str(), "dark" | "light" | "no_color") {
                return Err(ConfigError::Invalid(
                    "client.theme must be dark, light, or no_color".into(),
                ));
            }
            if !matches!(config.client.keymap.as_str(), "emacs" | "vim") {
                return Err(ConfigError::Invalid(
                    "client.keymap must be emacs or vim".into(),
                ));
            }
            if !matches!(
                config.client.statusline.as_str(),
                "full" | "compact" | "off"
            ) {
                return Err(ConfigError::Invalid(
                    "client.statusline must be full, compact, or off".into(),
                ));
            }
            if config.client.editor.as_ref().is_some_and(|editor| {
                editor.trim().is_empty() || editor.chars().any(|character| character.is_control())
            }) {
                return Err(ConfigError::Invalid(
                    "client.editor must be a non-empty command without control characters".into(),
                ));
            }
        }
        Component::ControlPlane => {
            config
                .control_plane
                .listen
                .parse::<SocketAddr>()
                .map_err(|_| {
                    ConfigError::Invalid("control_plane.listen must be a socket address".into())
                })?;
            let c = &config.control_plane;
            for (name, value) in [
                ("control_plane.database_url", &c.database_url),
                ("control_plane.database_provider", &c.database_provider),
                ("control_plane.nats_url", &c.nats_url),
                ("control_plane.nats_stream", &c.nats_stream),
                ("control_plane.oidc_issuer", &c.oidc_issuer),
                ("control_plane.oidc_audience", &c.oidc_audience),
                ("control_plane.oidc_jwks_uri", &c.oidc_jwks_uri),
                (
                    "control_plane.oidc_authorization_endpoint",
                    &c.oidc_authorization_endpoint,
                ),
                ("control_plane.oidc_token_endpoint", &c.oidc_token_endpoint),
                ("control_plane.oidc_client_id", &c.oidc_client_id),
                ("control_plane.oidc_redirect_uri", &c.oidc_redirect_uri),
                ("control_plane.identity_mode", &c.identity_mode),
                ("control_plane.organization_id", &c.organization_id),
                ("control_plane.organization_name", &c.organization_name),
                ("control_plane.team_id", &c.team_id),
                ("control_plane.team_name", &c.team_name),
                ("control_plane.admin_principal_id", &c.admin_principal_id),
                ("control_plane.bootstrap_device_id", &c.bootstrap_device_id),
                ("control_plane.policy_key_id", &c.policy_key_id),
                (
                    "control_plane.policy_public_key_base64",
                    &c.policy_public_key_base64,
                ),
                (
                    "control_plane.extension_scanner_key_id",
                    &c.extension_scanner_key_id,
                ),
                (
                    "control_plane.extension_scanner_public_key_base64",
                    &c.extension_scanner_public_key_base64,
                ),
            ] {
                required(name, value)?;
            }
            paired(
                &c.team_grant_key_id,
                &c.team_grant_private_key_base64,
                "control-plane Team grant signing key",
            )?;
            let audit_kms_values = [
                c.audit_kms_decrypt_url.is_some(),
                c.audit_kms_destroy_url.is_some(),
                c.audit_kms_credential_handle.is_some(),
            ];
            if audit_kms_values.iter().any(|configured| *configured)
                && !audit_kms_values.iter().all(|configured| *configured)
            {
                return Err(ConfigError::Invalid(
                    "control-plane audit-content KMS decrypt URL, destroy URL and credential handle must be configured together"
                        .into(),
                ));
            }
            if let Some(url) = c.audit_kms_decrypt_url.as_deref() {
                secure_or_loopback_url(url, "control_plane.audit_kms_decrypt_url")?;
            }
            if let Some(url) = c.audit_kms_destroy_url.as_deref() {
                secure_or_loopback_url(url, "control_plane.audit_kms_destroy_url")?;
            }
            if let Some(handle) = c.audit_kms_credential_handle.as_deref()
                && !valid_environment_name(handle)
            {
                return Err(ConfigError::Invalid(
                    "control_plane.audit_kms_credential_handle must be an environment credential name"
                        .into(),
                ));
            }
            let legacy_entitlement_values = [
                c.entitlement_key_id.is_some(),
                c.entitlement_public_key_base64.is_some(),
            ];
            if legacy_entitlement_values.iter().any(|value| *value)
                && !legacy_entitlement_values.iter().all(|value| *value)
            {
                return Err(ConfigError::Invalid(
                    "legacy control-plane entitlement key id and public key must be configured together"
                        .into(),
                ));
            }
            let legacy_entitlement = legacy_entitlement_values.iter().all(|value| *value);
            if legacy_entitlement && !c.entitlement_trust_roots.is_empty() {
                return Err(ConfigError::Invalid(
                    "legacy entitlement key fields cannot be combined with entitlement_trust_roots"
                        .into(),
                ));
            }
            if (legacy_entitlement || !c.entitlement_trust_roots.is_empty())
                != c.entitlement_token.is_some()
            {
                return Err(ConfigError::Invalid(
                    "control-plane entitlement trust roots and token must be configured together"
                        .into(),
                ));
            }
            validate_entitlement_trust_roots(&c.entitlement_trust_roots)?;
            if c.entitlement_token.is_some() {
                required("control_plane.entitlement_issuer", &c.entitlement_issuer)?;
            }
            required("control_plane.team_grant_issuer", &c.team_grant_issuer)?;
            required("control_plane.team_grant_audience", &c.team_grant_audience)?;
            if c.identity_mode != "oidc" && c.identity_mode != "saml_bridge" {
                return Err(ConfigError::Invalid(
                    "control_plane.identity_mode must be oidc or saml_bridge".into(),
                ));
            }
            if c.identity_mode == "saml_bridge" {
                required_option(
                    &c.saml_upstream_issuer,
                    "control_plane.saml_upstream_issuer",
                )?;
            }
            if c.nats_replicas == 0 {
                return Err(ConfigError::Invalid(
                    "control_plane.nats_replicas must be positive".into(),
                ));
            }
            if let Some(path) = c.nats_ca_file.as_deref()
                && (path.len() > 1024
                    || !std::path::Path::new(path).is_absolute()
                    || path.contains(['\n', '\r', '\0']))
            {
                return Err(ConfigError::Invalid(
                    "control_plane.nats_ca_file must be a bounded absolute path".into(),
                ));
            }
            if let Some(path) = c.oidc_ca_file.as_deref()
                && (path.len() > 1024
                    || !std::path::Path::new(path).is_absolute()
                    || path.contains(['\n', '\r', '\0']))
            {
                return Err(ConfigError::Invalid(
                    "control_plane.oidc_ca_file must be a bounded absolute path".into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_production(
    config: &RootConfig,
    component: Component,
    provenance: &BTreeMap<String, Provenance>,
) -> Result<(), ConfigError> {
    let serialized = serde_json::to_value(config).expect("configuration serializes");
    reject_control_characters(&serialized, "")?;

    for path in [
        "daemon.token",
        "daemon.runner_token",
        "daemon.storage_encryption_key_base64",
        "daemon.central_audit.private_key_base64",
        "control_plane.oidc_client_secret",
        "control_plane.team_grant_private_key_base64",
        "control_plane.entitlement_token",
        "runner.token",
        "client.token",
    ] {
        if get_path(&serialized, path).is_some_and(|value| !value.is_null()) {
            require_secret_source(path, provenance)?;
        }
    }
    for path in ["daemon.database_url", "control_plane.database_url"] {
        if get_path(&serialized, path)
            .and_then(Value::as_str)
            .is_some_and(url_contains_password)
        {
            require_secret_source(path, provenance)?;
        }
    }

    match component {
        Component::Daemon => {
            if config.daemon.auth_mode != "team_grant" {
                return Err(ConfigError::Invalid(
                    "production daemon.auth_mode must be team_grant".into(),
                ));
            }
            if config.daemon.token.is_some() {
                return Err(ConfigError::Invalid(
                    "production daemon.token is forbidden; use signed Team grants".into(),
                ));
            }
            required_option(&config.daemon.runner_token, "daemon.runner_token")?;
            required_option(
                &config.daemon.storage_encryption_key_id,
                "daemon.storage_encryption_key_id",
            )?;
            required_option(
                &config.daemon.storage_encryption_key_base64,
                "daemon.storage_encryption_key_base64",
            )?;
            if config.daemon.central_audit.enabled {
                require_secret_source("daemon.central_audit.private_key_base64", provenance)?;
            }
            let listen = config
                .daemon
                .listen
                .parse::<SocketAddr>()
                .expect("already validated");
            if !listen.ip().is_loopback() {
                return Err(ConfigError::Invalid(
                    "production daemon.listen must use a loopback address".into(),
                ));
            }
            if config.mcp.enabled
                && !config
                    .mcp
                    .config_path
                    .as_ref()
                    .is_some_and(|path| path.is_absolute())
            {
                return Err(ConfigError::Invalid(
                    "production mcp.config_path must be absolute".into(),
                ));
            }
            for (path, value) in [
                ("model.base_url", config.model.base_url.as_deref()),
                (
                    "connectors.github_api_base",
                    config.connectors.github_api_base.as_deref(),
                ),
                (
                    "connectors.jira_api_base",
                    config.connectors.jira_api_base.as_deref(),
                ),
                (
                    "connectors.servicenow_api_base",
                    config.connectors.servicenow_api_base.as_deref(),
                ),
                (
                    "connectors.slack_api_base",
                    config.connectors.slack_api_base.as_deref(),
                ),
                (
                    "connectors.splunk_hec_base_url",
                    config.connectors.splunk_hec_base_url.as_deref(),
                ),
            ] {
                if let Some(value) = value {
                    secure_or_loopback_url(value, path)?;
                }
            }
            for endpoint in &config.model.endpoints {
                secure_or_loopback_url(&endpoint.base_url, "model.endpoints[].base_url")?;
            }
        }
        Component::ControlPlane => {
            let c = &config.control_plane;
            required_option(&c.team_grant_key_id, "control_plane.team_grant_key_id")?;
            required_option(
                &c.team_grant_private_key_base64,
                "control_plane.team_grant_private_key_base64",
            )?;
            if c.entitlement_trust_roots.is_empty() {
                required_option(&c.entitlement_key_id, "control_plane.entitlement_key_id")?;
                required_option(
                    &c.entitlement_public_key_base64,
                    "control_plane.entitlement_public_key_base64",
                )?;
            }
            required_option(&c.entitlement_token, "control_plane.entitlement_token")?;
            required("control_plane.entitlement_issuer", &c.entitlement_issuer)?;
            for (path, value) in [
                ("control_plane.oidc_issuer", c.oidc_issuer.as_str()),
                ("control_plane.oidc_jwks_uri", c.oidc_jwks_uri.as_str()),
                (
                    "control_plane.oidc_authorization_endpoint",
                    c.oidc_authorization_endpoint.as_str(),
                ),
                (
                    "control_plane.oidc_token_endpoint",
                    c.oidc_token_endpoint.as_str(),
                ),
                (
                    "control_plane.oidc_redirect_uri",
                    c.oidc_redirect_uri.as_str(),
                ),
            ] {
                secure_or_loopback_url(value, path)?;
            }
            if let Some(value) = c.audit_kms_decrypt_url.as_deref() {
                secure_or_loopback_url(value, "control_plane.audit_kms_decrypt_url")?;
            }
            if let Some(value) = c.audit_kms_destroy_url.as_deref() {
                secure_or_loopback_url(value, "control_plane.audit_kms_destroy_url")?;
            }
            for (path, value) in [
                ("control_plane.organization_id", c.organization_id.as_str()),
                ("control_plane.team_id", c.team_id.as_str()),
                (
                    "control_plane.admin_principal_id",
                    c.admin_principal_id.as_str(),
                ),
                (
                    "control_plane.bootstrap_device_id",
                    c.bootstrap_device_id.as_str(),
                ),
                ("control_plane.policy_key_id", c.policy_key_id.as_str()),
                (
                    "control_plane.extension_scanner_key_id",
                    c.extension_scanner_key_id.as_str(),
                ),
            ] {
                production_identifier(path, value)?;
            }
            if let Some(key_id) = c.entitlement_key_id.as_deref() {
                production_identifier("control_plane.entitlement_key_id", key_id)?;
            }
            for root in &c.entitlement_trust_roots {
                production_identifier(
                    "control_plane.entitlement_trust_roots.key_id",
                    &root.key_id,
                )?;
            }
        }
        Component::Runner => {
            if let Some(worker_id) = config.runner.worker_id.as_deref() {
                production_identifier("runner.worker_id", worker_id)?;
            }
        }
        Component::Cli => {
            for (path, value) in [
                (
                    "client.organization_id",
                    config.client.organization_id.as_str(),
                ),
                ("client.team_id", config.client.team_id.as_str()),
                ("client.actor_id", config.client.actor_id.as_str()),
            ] {
                production_identifier(path, value)?;
            }
        }
    }
    for (path, handle) in [
        (
            "model.credential_handle",
            config.model.credential_handle.as_deref(),
        ),
        (
            "connectors.github_credential_handle",
            config.connectors.github_credential_handle.as_deref(),
        ),
        (
            "connectors.ci_credential_handle",
            config.connectors.ci_credential_handle.as_deref(),
        ),
        (
            "connectors.jira_credential_handle",
            config.connectors.jira_credential_handle.as_deref(),
        ),
        (
            "connectors.servicenow_credential_handle",
            config.connectors.servicenow_credential_handle.as_deref(),
        ),
        (
            "connectors.slack_credential_handle",
            config.connectors.slack_credential_handle.as_deref(),
        ),
        (
            "connectors.teams_webhook_handle",
            config.connectors.teams_webhook_handle.as_deref(),
        ),
        (
            "connectors.splunk_credential_handle",
            config.connectors.splunk_credential_handle.as_deref(),
        ),
        (
            "control_plane.audit_kms_credential_handle",
            config.control_plane.audit_kms_credential_handle.as_deref(),
        ),
    ] {
        if let Some(handle) = handle
            && !valid_environment_name(handle)
        {
            return Err(ConfigError::Invalid(format!(
                "production credential handle {path} must be an uppercase environment name"
            )));
        }
    }
    for endpoint in &config.model.endpoints {
        if let Some(handle) = endpoint.credential_handle.as_deref()
            && !valid_environment_name(handle)
        {
            return Err(ConfigError::Invalid(format!(
                "production credential handle model.endpoints[{}].credential_handle must be an uppercase environment name",
                endpoint.id
            )));
        }
    }
    Ok(())
}

fn production_identifier(path: &str, value: &str) -> Result<(), ConfigError> {
    if value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
    {
        return Err(ConfigError::Invalid(format!(
            "production identifier {path} must be at most 128 ASCII letters, digits, '.', '-', '_' or ':'"
        )));
    }
    Ok(())
}

fn require_secret_source(
    path: &str,
    provenance: &BTreeMap<String, Provenance>,
) -> Result<(), ConfigError> {
    if provenance.get(path).map(|value| &value.source) == Some(&SourceKind::SecretEnvironment) {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!(
            "production sensitive field {path} must use an explicit secret_references entry"
        )))
    }
}

fn reject_control_characters(value: &Value, path: &str) -> Result<(), ConfigError> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                reject_control_characters(value, &child)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_control_characters(value, path)?;
            }
        }
        Value::String(value) if value.chars().any(char::is_control) => {
            return Err(ConfigError::Invalid(format!(
                "production field {path} contains control characters"
            )));
        }
        Value::String(value) if value.len() > 16 * 1024 => {
            return Err(ConfigError::Invalid(format!(
                "production field {path} exceeds 16 KiB"
            )));
        }
        _ => {}
    }
    Ok(())
}

fn required(name: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        Err(ConfigError::Invalid(format!("{name} is required")))
    } else {
        Ok(())
    }
}
fn required_option(value: &Option<String>, name: &str) -> Result<(), ConfigError> {
    value.as_deref().map_or_else(
        || Err(ConfigError::Invalid(format!("{name} is required"))),
        |value| required(name, value),
    )
}
fn paired(left: &Option<String>, right: &Option<String>, name: &str) -> Result<(), ConfigError> {
    if left.is_some() == right.is_some() {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!(
            "{name} id and value must be configured together"
        )))
    }
}
fn secure_or_loopback_url(raw: &str, name: &str) -> Result<(), ConfigError> {
    let url =
        url::Url::parse(raw).map_err(|error| ConfigError::Invalid(format!("{name}: {error}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::Invalid(format!(
            "{name} must not contain URL userinfo"
        )));
    }
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!(
            "{name} must use HTTPS or loopback HTTP"
        )))
    }
}

fn merge(
    target: &mut Value,
    source: Value,
    prefix: &str,
    kind: SourceKind,
    detail: &str,
    provenance: &mut BTreeMap<String, Provenance>,
) -> Result<(), ConfigError> {
    let source = source
        .as_object()
        .ok_or_else(|| ConfigError::Invalid("configuration root must be an object".into()))?;
    let target = target
        .as_object_mut()
        .expect("default configuration is object");
    for (key, value) in source {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let Some(existing) = target.get_mut(key) else {
            if prefix == "secret_references" {
                target.insert(key.clone(), value.clone());
                provenance.insert(
                    path,
                    Provenance {
                        source: kind.clone(),
                        detail: detail.into(),
                    },
                );
                continue;
            }
            return Err(ConfigError::Invalid(format!(
                "unknown configuration field {path}"
            )));
        };
        if value.is_object() && existing.is_object() {
            merge(
                existing,
                value.clone(),
                &path,
                kind.clone(),
                detail,
                provenance,
            )?;
        } else {
            *existing = value.clone();
            provenance.insert(
                path,
                Provenance {
                    source: kind.clone(),
                    detail: detail.into(),
                },
            );
        }
    }
    Ok(())
}

fn record_leaves(
    value: &Value,
    prefix: &str,
    kind: SourceKind,
    detail: &str,
    output: &mut BTreeMap<String, Provenance>,
) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            record_leaves(value, &path, kind.clone(), detail, output);
        }
    } else {
        output.insert(
            prefix.into(),
            Provenance {
                source: kind,
                detail: detail.into(),
            },
        );
    }
}

fn set_path(root: &mut Value, path: &str, value: Value) {
    let parts = path.split('.').collect::<Vec<_>>();
    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        current = current
            .as_object_mut()
            .expect("configuration object")
            .entry(*part)
            .or_insert_with(|| Value::Object(Map::new()));
    }
    current
        .as_object_mut()
        .expect("configuration object")
        .insert(parts[parts.len() - 1].into(), value);
}
fn get_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(root, |value, part| value.get(part))
}

fn apply_secret_references(
    root: &mut Value,
    environment: &BTreeMap<String, String>,
    provenance: &mut BTreeMap<String, Provenance>,
) -> Result<(), ConfigError> {
    let references = root
        .get("secret_references")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for (path, variable) in references {
        if !SENSITIVE_PATHS.contains(&path.as_str()) {
            return Err(ConfigError::Invalid(format!(
                "secret_references contains unsupported target {path}"
            )));
        }
        let variable = variable.as_str().ok_or_else(|| {
            ConfigError::Invalid(format!(
                "secret reference {path} must be an environment name"
            ))
        })?;
        if !valid_environment_name(variable) {
            return Err(ConfigError::Invalid(format!(
                "secret reference {path} has an invalid environment name"
            )));
        }
        let secret = environment.get(variable).ok_or_else(|| {
            ConfigError::Invalid(format!(
                "secret environment variable {variable} referenced by {path} is missing"
            ))
        })?;
        if secret.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "secret environment variable {variable} referenced by {path} is empty"
            )));
        }
        set_path(root, &path, Value::String(secret.clone()));
        provenance.insert(
            path,
            Provenance {
                source: SourceKind::SecretEnvironment,
                detail: variable.into(),
            },
        );
    }
    Ok(())
}

fn valid_environment_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn reject_exposed_secret_file(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
    value: &Value,
) -> Result<(), ConfigError> {
    let contains_secret = [
        "daemon.token",
        "daemon.runner_token",
        "daemon.storage_encryption_key_base64",
        "daemon.central_audit.private_key_base64",
        "control_plane.oidc_client_secret",
        "control_plane.team_grant_private_key_base64",
        "control_plane.entitlement_token",
        "runner.token",
        "client.token",
    ]
    .iter()
    .any(|field| get_path(value, field).is_some())
        || ["daemon.database_url", "control_plane.database_url"]
            .iter()
            .filter_map(|field| get_path(value, field).and_then(Value::as_str))
            .any(url_contains_password);
    #[cfg(unix)]
    if contains_secret {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ConfigError::Invalid(format!(
                "{} contains sensitive fields and must not be readable by group or others (use mode 0600 or stricter)",
                path.display()
            )));
        }
    }
    let _ = (path, metadata, contains_secret);
    Ok(())
}

fn reject_mutable_production_file(
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
) -> Result<(), ConfigError> {
    if !metadata.is_file() {
        return Err(ConfigError::Invalid(format!(
            "production configuration {} must be a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(ConfigError::Invalid(format!(
                "production configuration {} must not be writable by group or others",
                path.display()
            )));
        }
    }
    Ok(())
}

fn url_contains_password(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| url.password().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_daemon_connection_is_private_atomic_and_process_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("run").join("daemon.json");
        let connection = LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "generated-local-token".into(),
        );
        let guard = publish_local_daemon_connection_to(&connection, path.clone()).unwrap();
        assert_eq!(
            read_local_daemon_connection_from(&path).unwrap(),
            connection
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        drop(guard);
        assert!(!path.exists());
    }

    #[test]
    fn local_daemon_connection_rejects_remote_and_exposed_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("daemon.json");
        let remote = LocalDaemonConnection::new(
            "https://daemon.example".into(),
            "generated-local-token".into(),
        );
        assert!(
            publish_local_daemon_connection_to(&remote, path.clone())
                .unwrap_err()
                .contains("loopback")
        );
        let local = LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "generated-local-token".into(),
        );
        let guard = publish_local_daemon_connection_to(&local, path.clone()).unwrap();
        std::mem::forget(guard);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                read_local_daemon_connection_from(&path)
                    .unwrap_err()
                    .contains("0600")
            );
        }
    }

    fn valid_control_plane_config() -> RootConfig {
        let mut config = RootConfig::default();
        let control = &mut config.control_plane;
        control.database_url = "postgresql://localhost/opencoding".into();
        control.nats_url = "nats://127.0.0.1:4222".into();
        control.oidc_issuer = "https://id.example".into();
        control.oidc_audience = "opencoding".into();
        control.oidc_jwks_uri = "https://id.example/jwks".into();
        control.oidc_authorization_endpoint = "https://id.example/authorize".into();
        control.oidc_token_endpoint = "https://id.example/token".into();
        control.oidc_client_id = "opencoding".into();
        control.oidc_redirect_uri = "https://control.example/auth/callback".into();
        control.organization_id = "org-a".into();
        control.organization_name = "Organization A".into();
        control.team_id = "team-a".into();
        control.team_name = "Team A".into();
        control.admin_principal_id = "admin-a".into();
        control.bootstrap_device_id = "device-a".into();
        control.policy_key_id = "policy-a".into();
        control.policy_public_key_base64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
        control.extension_scanner_key_id = "extension-scanner-a".into();
        control.extension_scanner_public_key_base64 =
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
        config
    }

    #[test]
    fn precedence_and_provenance_are_deterministic() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"runner":{"token":"file-token","lease_seconds":45}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let effective = ConfigLoader::new()
            .with_file(&path)
            .with_environment([("OPENCODING_RUNNER_TOKEN", "env-token")])
            .with_cli_override("runner.lease_seconds", Value::from(60))
            .load(Component::Runner)
            .unwrap();
        assert_eq!(effective.config.runner.token.as_deref(), Some("env-token"));
        assert_eq!(effective.config.runner.lease_seconds, 60);
        assert_eq!(
            effective.provenance("runner.token").unwrap().source,
            SourceKind::Environment
        );
        assert_eq!(
            effective.provenance("runner.lease_seconds").unwrap().source,
            SourceKind::Cli
        );
    }

    #[test]
    fn effective_output_redacts_secrets() {
        let effective = ConfigLoader::new()
            .with_environment([
                ("OPENCODING_TOKEN", "super-secret"),
                ("OPENCODING_URL", "http://127.0.0.1:4096"),
            ])
            .load(Component::Cli)
            .unwrap();
        let text = serde_json::to_string(&effective.redacted_json()).unwrap();
        assert!(!text.contains("super-secret"));
        assert_eq!(effective.redacted_json()["client"]["token"], "[REDACTED]");
        assert!(effective.redacted_json()["runner"]["token"].is_null());
    }

    #[test]
    fn control_plane_entitlement_configuration_is_atomic_and_redacted() {
        let mut config = valid_control_plane_config();
        config.control_plane.entitlement_key_id = Some("license-root-1".into());
        let error = validate(&config, Component::ControlPlane).unwrap_err();
        assert!(error.to_string().contains("must be configured together"));

        config.control_plane.entitlement_public_key_base64 =
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
        config.control_plane.entitlement_token = Some("sensitive-license-token".into());
        validate(&config, Component::ControlPlane).unwrap();
        let effective = EffectiveConfig {
            config,
            provenance: BTreeMap::new(),
        };
        assert_eq!(
            effective.redacted_json()["control_plane"]["entitlement_token"],
            "[REDACTED]"
        );
    }

    #[test]
    fn control_plane_audit_kms_lifecycle_is_atomic_and_transport_safe() {
        let mut config = valid_control_plane_config();
        config.control_plane.audit_kms_decrypt_url = Some("https://kms.example/v1/decrypt".into());
        let error = validate(&config, Component::ControlPlane).unwrap_err();
        assert!(error.to_string().contains("must be configured together"));

        config.control_plane.audit_kms_destroy_url = Some("https://kms.example/v1/destroy".into());
        config.control_plane.audit_kms_credential_handle = Some("AUDIT_KMS_TOKEN".into());
        validate(&config, Component::ControlPlane).unwrap();

        config.control_plane.audit_kms_destroy_url = Some("http://kms.example/destroy".into());
        let error = validate(&config, Component::ControlPlane).unwrap_err();
        assert!(error.to_string().contains("HTTPS or loopback"));

        config.control_plane.audit_kms_destroy_url = Some("http://127.0.0.1:9000/destroy".into());
        config.control_plane.audit_kms_credential_handle = Some("raw-secret-token".into());
        let error = validate(&config, Component::ControlPlane).unwrap_err();
        assert!(error.to_string().contains("environment credential name"));
    }

    #[test]
    fn entitlement_trust_root_rotation_is_structural_and_cannot_mix_legacy_fields() {
        let mut config = valid_control_plane_config();
        config.control_plane.entitlement_token = Some("signed-token".into());
        config.control_plane.entitlement_trust_roots = vec![
            EntitlementTrustRootConfig {
                key_id: "license-root-2026-02".into(),
                public_key_base64: "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".into(),
                generation: 2,
                status: EntitlementTrustRootStatusConfig::Active,
                accept_until: None,
            },
            EntitlementTrustRootConfig {
                key_id: "license-root-2026-01".into(),
                public_key_base64: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                generation: 1,
                status: EntitlementTrustRootStatusConfig::Retiring,
                accept_until: Some(Utc::now() + chrono::Duration::days(30)),
            },
        ];
        validate(&config, Component::ControlPlane).unwrap();

        config.control_plane.entitlement_key_id = Some("legacy-root".into());
        config.control_plane.entitlement_public_key_base64 =
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
        assert!(
            validate(&config, Component::ControlPlane)
                .unwrap_err()
                .to_string()
                .contains("cannot be combined")
        );
        config.control_plane.entitlement_key_id = None;
        config.control_plane.entitlement_public_key_base64 = None;
        config.control_plane.entitlement_trust_roots[0].status =
            EntitlementTrustRootStatusConfig::Retiring;
        config.control_plane.entitlement_trust_roots[0].accept_until =
            Some(Utc::now() + chrono::Duration::days(60));
        assert!(validate(&config, Component::ControlPlane).is_err());
    }

    #[test]
    fn production_control_plane_requires_secret_sourced_entitlement() {
        let mut config = valid_control_plane_config();
        config.profile = Profile::Production;
        config.control_plane.team_grant_key_id = Some("team-grant-1".into());
        config.control_plane.team_grant_private_key_base64 = Some("private-key".into());
        let mut provenance = BTreeMap::from([(
            "control_plane.team_grant_private_key_base64".into(),
            Provenance {
                source: SourceKind::SecretEnvironment,
                detail: "TEAM_GRANT_PRIVATE_KEY".into(),
            },
        )]);
        let error = validate_production(&config, Component::ControlPlane, &provenance).unwrap_err();
        assert!(error.to_string().contains("entitlement_key_id"));

        config.control_plane.entitlement_key_id = Some("license-root-1".into());
        config.control_plane.entitlement_public_key_base64 =
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
        config.control_plane.entitlement_token = Some("signed-token".into());
        provenance.insert(
            "control_plane.entitlement_token".into(),
            Provenance {
                source: SourceKind::SecretEnvironment,
                detail: "ENTITLEMENT_TOKEN".into(),
            },
        );
        validate(&config, Component::ControlPlane).unwrap();
        validate_production(&config, Component::ControlPlane, &provenance).unwrap();
    }

    #[test]
    fn control_plane_private_ca_files_must_use_absolute_bounded_paths() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = valid_control_plane_config();
        config.control_plane.nats_ca_file = Some("relative/ca.pem".into());
        assert!(
            validate(&config, Component::ControlPlane)
                .unwrap_err()
                .to_string()
                .contains("nats_ca_file")
        );
        config.control_plane.nats_ca_file = Some(
            directory
                .path()
                .join("opencoding-nats-ca.crt")
                .to_string_lossy()
                .into_owned(),
        );
        validate(&config, Component::ControlPlane).unwrap();
        config.control_plane.oidc_ca_file = Some("relative/oidc-ca.pem".into());
        assert!(
            validate(&config, Component::ControlPlane)
                .unwrap_err()
                .to_string()
                .contains("oidc_ca_file")
        );
        config.control_plane.oidc_ca_file = Some(
            directory
                .path()
                .join("opencoding-oidc-ca.crt")
                .to_string_lossy()
                .into_owned(),
        );
        validate(&config, Component::ControlPlane).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_and_insecure_remote_urls() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, r#"{"client":{"tokne":"x"}}"#).unwrap();
        assert!(
            ConfigLoader::new()
                .with_file(&path)
                .load(Component::Cli)
                .unwrap_err()
                .to_string()
                .contains("unknown configuration field")
        );
        let error = ConfigLoader::new()
            .with_environment([
                ("OPENCODING_TOKEN", "x"),
                ("OPENCODING_URL", "http://example.com"),
            ])
            .load(Component::Cli)
            .unwrap_err();
        assert!(error.to_string().contains("HTTPS"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_readable_secret_files() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[client]\ntoken = 'secret'\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = ConfigLoader::new()
            .with_file(&path)
            .load(Component::Cli)
            .unwrap_err();
        assert!(error.to_string().contains("0600"));
    }

    #[test]
    fn production_ignores_general_environment_and_resolves_only_explicit_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("production.toml");
        std::fs::write(
            &path,
            r#"
profile = "production"

[daemon]
listen = "127.0.0.1:0"
database_url = "sqlite://production.db"
auth_mode = "team_grant"
team_grant_key_id = "grant-key-1"
team_grant_public_key_base64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
storage_encryption_key_id = "storage-key-1"

[secret_references]
"daemon.runner_token" = "RUNNER_TOKEN_FROM_SECRET_STORE"
"daemon.storage_encryption_key_base64" = "STORAGE_KEY_FROM_SECRET_STORE"
"#,
        )
        .unwrap();
        let effective = ConfigLoader::new()
            .with_file(&path)
            .with_environment([
                ("RUNNER_TOKEN_FROM_SECRET_STORE", "trusted-runner-token"),
                (
                    "STORAGE_KEY_FROM_SECRET_STORE",
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                ),
                ("OPENCODING_TOKEN", "injected-token"),
                ("OPENCODING_DATABASE_URL", "sqlite://injected.db"),
            ])
            .load(Component::Daemon)
            .unwrap();
        assert!(effective.config.daemon.token.is_none());
        assert_eq!(
            effective.config.daemon.runner_token.as_deref(),
            Some("trusted-runner-token")
        );
        assert_eq!(
            effective.config.daemon.database_url,
            "sqlite://production.db"
        );
        assert_eq!(
            effective
                .provenance("daemon.storage_encryption_key_base64")
                .unwrap()
                .source,
            SourceKind::SecretEnvironment
        );
    }

    #[test]
    fn production_rejects_literal_secrets_and_cli_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("production.toml");
        std::fs::write(
            &path,
            "profile = 'production'\n[daemon]\ntoken = 'literal-secret'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = ConfigLoader::new()
            .with_file(&path)
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("secret_references"));

        std::fs::write(
            &path,
            "profile = 'production'\n[secret_references]\n'daemon.token' = 'TOKEN'\n",
        )
        .unwrap();
        let error = ConfigLoader::new()
            .with_file(&path)
            .with_environment([("TOKEN", "trusted")])
            .with_cli_override(
                "daemon.database_url",
                Value::String("sqlite://other.db".into()),
            )
            .load(Component::Daemon)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CLI field overrides are disabled")
        );
    }

    #[test]
    fn production_forbids_global_daemon_token() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("production.toml");
        std::fs::write(
            &path,
            r#"
profile = "production"

[daemon]
listen = "127.0.0.1:0"
database_url = "sqlite://production.db"
auth_mode = "team_grant"
team_grant_key_id = "grant-key-1"
team_grant_public_key_base64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
storage_encryption_key_id = "storage-key-1"

[secret_references]
"daemon.token" = "SHARED_TOKEN"
"daemon.runner_token" = "RUNNER_TOKEN"
"daemon.storage_encryption_key_base64" = "STORAGE_KEY"
"#,
        )
        .unwrap();
        let error = ConfigLoader::new()
            .with_file(&path)
            .with_environment([
                ("SHARED_TOKEN", "forbidden-global-token"),
                ("RUNNER_TOKEN", "runner-secret"),
                ("STORAGE_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            ])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("daemon.token is forbidden"));
    }

    #[test]
    fn central_audit_export_configuration_is_atomic_scoped_and_transport_safe() {
        let mut config = RootConfig::default();
        config.daemon.central_audit.enabled = true;
        assert!(
            validate(&config, Component::Daemon)
                .unwrap_err()
                .to_string()
                .contains("ingest_url")
        );
        config.daemon.central_audit = CentralAuditConfig {
            enabled: true,
            ingest_url: Some("http://127.0.0.1:8443/v1/organizations/org/audit/ingest".into()),
            source_id: Some("source-a".into()),
            organization_id: Some("org-a".into()),
            team_id: Some("team-a".into()),
            key_id: Some("audit-key-a".into()),
            private_key_base64: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()),
            kms_generate_data_key_url: None,
            kms_credential_handle: None,
        };
        validate(&config, Component::Daemon).unwrap();
        config.daemon.central_audit.kms_generate_data_key_url =
            Some("http://127.0.0.1:9443/v1/data-keys".into());
        assert!(
            validate(&config, Component::Daemon)
                .unwrap_err()
                .to_string()
                .contains("GenerateDataKey provider")
        );
        config.daemon.central_audit.kms_credential_handle = Some("AUDIT_KMS_TOKEN".into());
        validate(&config, Component::Daemon).unwrap();
        config.daemon.central_audit.kms_generate_data_key_url =
            Some("http://kms.example/v1/data-keys".into());
        assert!(
            validate(&config, Component::Daemon)
                .unwrap_err()
                .to_string()
                .contains("HTTPS")
        );
        config.daemon.central_audit.kms_generate_data_key_url = None;
        config.daemon.central_audit.kms_credential_handle = None;
        config.daemon.central_audit.ingest_url =
            Some("http://control-plane.example/v1/organizations/org/audit/ingest".into());
        assert!(
            validate(&config, Component::Daemon)
                .unwrap_err()
                .to_string()
                .contains("HTTPS")
        );
        config.daemon.central_audit.enabled = false;
        assert!(
            validate(&config, Component::Daemon)
                .unwrap_err()
                .to_string()
                .contains("enabled = true")
        );
    }

    #[test]
    fn model_provider_selection_is_explicit_and_atomic() {
        let effective = ConfigLoader::new()
            .with_environment([
                ("OPENCODING_MODEL_PROVIDER", "anthropic"),
                ("OPENCODING_MODEL_BASE_URL", "https://api.anthropic.com/v1"),
                ("OPENCODING_MODEL_CREDENTIAL_HANDLE", "ANTHROPIC_API_KEY"),
            ])
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(effective.config.model.provider, "anthropic");

        let error = ConfigLoader::new()
            .with_environment([("OPENCODING_MODEL_PROVIDER", "unknown")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("openai_compatible"));

        ConfigLoader::new()
            .with_environment([(
                "OPENCODING_MODEL_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta",
            )])
            .load(Component::Daemon)
            .unwrap();

        let configured = ConfigLoader::new()
            .with_environment([("OPENCODING_MODEL_CREDENTIAL_HANDLE", "GEMINI_API_KEY")])
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(
            configured.config.model.base_url.as_deref(),
            Some(DEFAULT_MODEL_API_BASE_URL)
        );
        assert_eq!(
            configured.config.model.credential_handle.as_deref(),
            Some("GEMINI_API_KEY")
        );

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("routed-models.toml");
        std::fs::write(
            &path,
            r#"
[model]
[[model.endpoints]]
id = "primary"
provider = "anthropic"
provider_model = "claude-primary"
base_url = "https://api.anthropic.com/v1"
credential_handle = "ANTHROPIC_API_KEY"

[[model.endpoints]]
id = "fallback"
provider = "gemini"
provider_model = "gemini-fallback"
base_url = "https://generativelanguage.googleapis.com/v1beta"
credential_handle = "GEMINI_API_KEY"
"#,
        )
        .unwrap();
        let routed = ConfigLoader::new()
            .with_file(&path)
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(routed.config.model.endpoints.len(), 2);
        assert_eq!(routed.config.model.endpoints[1].id, "fallback");

        std::fs::write(
            &path,
            r#"
[model]
base_url = "https://api.example/v1"
[[model.endpoints]]
id = "duplicate-mode"
provider = "openai_compatible"
provider_model = "model"
base_url = "https://api.example/v1"
"#,
        )
        .unwrap();
        let error = ConfigLoader::new()
            .with_file(&path)
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("cannot be combined"));
    }

    #[test]
    fn cli_interface_settings_are_typed_validated_and_environment_configurable() {
        let configured = ConfigLoader::new()
            .with_environment([
                ("OPENCODING_THEME", "light"),
                ("OPENCODING_KEYMAP", "vim"),
                ("OPENCODING_STATUSLINE", "compact"),
                ("OPENCODING_EDITOR", "code --wait"),
            ])
            .load(Component::Cli)
            .unwrap()
            .config
            .client;
        assert_eq!(configured.theme, "light");
        assert_eq!(configured.keymap, "vim");
        assert_eq!(configured.statusline, "compact");
        assert_eq!(configured.editor.as_deref(), Some("code --wait"));

        for (environment, value, expected) in [
            ("OPENCODING_THEME", "sepia", "client.theme"),
            ("OPENCODING_KEYMAP", "random", "client.keymap"),
            ("OPENCODING_STATUSLINE", "verbose", "client.statusline"),
        ] {
            let error = ConfigLoader::new()
                .with_environment([(environment, value)])
                .load(Component::Cli)
                .unwrap_err();
            assert!(error.to_string().contains(expected));
        }
    }
}
