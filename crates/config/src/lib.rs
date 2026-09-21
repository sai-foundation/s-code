//! Versioned, provenance-aware configuration for every S-Code process.
pub mod onboarding;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_MODEL_API_BASE_URL: &str = "http://127.0.0.1:18787/v1";
pub const LOCAL_DAEMON_CONNECTION_SCHEMA_VERSION: u32 = 2;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_CONNECTION_BYTES: u64 = 16 * 1024;

pub fn s_code_home_directory() -> Result<PathBuf, String> {
    s_code_home_directory_from(|name| std::env::var_os(name))
}

pub fn local_state_directory() -> Result<PathBuf, String> {
    local_state_directory_from(|name| std::env::var_os(name))
}

pub fn local_product_directories() -> Result<Vec<PathBuf>, String> {
    let runtime = local_daemon_connection_path()?
        .parent()
        .ok_or("local daemon connection path has no parent")?
        .to_path_buf();
    let mut directories = vec![s_code_home_directory()?, local_state_directory()?, runtime];
    directories.sort();
    directories.dedup();
    Ok(directories)
}

pub fn default_user_config_path() -> Result<PathBuf, String> {
    Ok(s_code_home_directory()?.join("config.toml"))
}

fn s_code_home_directory_from(value: impl Fn(&str) -> Option<OsString>) -> Result<PathBuf, String> {
    if let Some(directory) = value("S_CODE_HOME") {
        if directory.is_empty() {
            return Err("S_CODE_HOME must not be empty".into());
        }
        let directory = PathBuf::from(directory);
        validate_absolute_directory_override("S_CODE_HOME", &directory)?;
        return Ok(directory);
    }
    let home = value("HOME")
        .or_else(|| value("USERPROFILE"))
        .ok_or("HOME or USERPROFILE is unavailable")?;
    if home.is_empty() {
        return Err("HOME must not be empty".into());
    }
    let home = PathBuf::from(home);
    validate_absolute_directory_override("HOME", &home)?;
    Ok(home.join(".s-code"))
}

fn local_state_directory_from(
    value: impl Fn(&str) -> Option<OsString> + Copy,
) -> Result<PathBuf, String> {
    if let Some(directory) = value("S_CODE_STATE_DIR") {
        if directory.is_empty() {
            return Err("S_CODE_STATE_DIR must not be empty".into());
        }
        let directory = PathBuf::from(directory);
        validate_absolute_directory_override("S_CODE_STATE_DIR", &directory)?;
        return Ok(directory);
    }
    Ok(s_code_home_directory_from(value)?.join("state"))
}

fn validate_absolute_directory_override(label: &str, directory: &Path) -> Result<(), String> {
    if !directory.is_absolute()
        || directory.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(format!(
            "{label} must be a normalized absolute path without . or .. components"
        ));
    }
    Ok(())
}

fn sqlite_url(path: &Path) -> Result<String, ConfigError> {
    let path = path
        .to_str()
        .ok_or_else(|| ConfigError::Invalid("local state path must contain valid UTF-8".into()))?;
    Ok(format!("sqlite://{path}"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDaemonConnection {
    pub schema_version: u32,
    pub daemon_url: String,
    pub token: String,
    pub instance_id: String,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
}

impl LocalDaemonConnection {
    pub fn new(daemon_url: String, token: String, instance_id: String) -> Self {
        Self {
            schema_version: LOCAL_DAEMON_CONNECTION_SCHEMA_VERSION,
            daemon_url,
            token,
            instance_id,
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
            || self.instance_id.len() < 16
            || self.instance_id.len() > 128
            || !self
                .instance_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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

#[derive(Debug)]
pub struct LocalDaemonInstanceGuard {
    file: std::fs::File,
}

impl Drop for LocalDaemonInstanceGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
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
    local_daemon_connection_path_from(|name| std::env::var_os(name))
}

fn local_daemon_connection_path_from(
    value: impl Fn(&str) -> Option<OsString> + Copy,
) -> Result<PathBuf, String> {
    if let Some(directory) = value("S_CODE_RUNTIME_DIR") {
        if directory.is_empty() {
            return Err("S_CODE_RUNTIME_DIR must not be empty".into());
        }
        let directory = PathBuf::from(directory);
        validate_absolute_directory_override("S_CODE_RUNTIME_DIR", &directory)?;
        return Ok(directory.join("daemon.json"));
    }
    Ok(s_code_home_directory_from(value)?
        .join("run")
        .join("daemon.json"))
}

pub fn acquire_local_daemon_instance() -> Result<LocalDaemonInstanceGuard, String> {
    let connection_path = local_daemon_connection_path()?;
    let directory = connection_path
        .parent()
        .ok_or("local daemon connection path has no parent")?;
    ensure_private_runtime_directory(directory)?;
    let path = directory.join("daemon.lock");
    if std::fs::symlink_metadata(&path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("local daemon instance lock must not be a symbolic link".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    file.try_lock().map_err(|error| {
        format!(
            "another local S-Code service already owns {}: {error}",
            path.display()
        )
    })?;
    file.set_len(0)
        .map_err(|error| format!("cannot reset {}: {error}", path.display()))?;
    use std::io::Write as _;
    writeln!(file, "{}", std::process::id())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))?;
    Ok(LocalDaemonInstanceGuard { file })
}

pub fn read_local_daemon_connection() -> Result<LocalDaemonConnection, String> {
    read_local_daemon_connection_from(&local_daemon_connection_path()?)
}

pub fn recorded_local_daemon_owns_instance_lock() -> Result<bool, String> {
    let connection = read_local_daemon_connection()?;
    let connection_path = local_daemon_connection_path()?;
    let lock_path = connection_path
        .parent()
        .ok_or("local daemon connection path has no parent")?
        .join("daemon.lock");
    let metadata = std::fs::symlink_metadata(&lock_path)
        .map_err(|error| format!("cannot inspect {}: {error}", lock_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 64 {
        return Err("local daemon instance lock must be a small regular file".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?;
    let mut recorded_pid = String::new();
    file.read_to_string(&mut recorded_pid)
        .map_err(|error| format!("cannot read {}: {error}", lock_path.display()))?;
    let recorded_pid = recorded_pid
        .trim()
        .parse::<u32>()
        .map_err(|_| "local daemon instance lock contains an invalid PID")?;
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            Ok(false)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(recorded_pid == connection.pid),
        Err(std::fs::TryLockError::Error(error)) => Err(format!(
            "cannot verify {} ownership: {error}",
            lock_path.display()
        )),
    }
}

pub fn local_daemon_instance_lock_is_free() -> Result<bool, String> {
    let connection_path = local_daemon_connection_path()?;
    let lock_path = connection_path
        .parent()
        .ok_or("local daemon connection path has no parent")?
        .join("daemon.lock");
    let metadata = std::fs::symlink_metadata(&lock_path)
        .map_err(|error| format!("cannot inspect {}: {error}", lock_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("local daemon instance lock must be a regular file".into());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            Ok(true)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(error)) => Err(format!(
            "cannot verify {} availability: {error}",
            lock_path.display()
        )),
    }
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
    ensure_private_runtime_directory(directory)?;
    let directory_metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err("local daemon runtime directory must be a regular directory".into());
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

fn ensure_private_runtime_directory(directory: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let existed = directory.exists();
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let metadata = std::fs::symlink_metadata(directory)
        .map_err(|error| format!("cannot inspect {}: {error}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("local daemon runtime directory must be a regular directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if existed && metadata.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "existing local daemon runtime directory {} must already use mode 0700 or stricter",
                directory.display()
            ));
        }
        if !existed {
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("cannot secure {}: {error}", directory.display()))?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Component {
    Daemon,
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
    /// Verified experience memory: `off` (default), `observe` (record
    /// quarantined candidates only) or `verified` (also retrieve approved
    /// experiences). Evaluation-only until the controlled experiment passes.
    pub experience_mode: String,
    /// Experience promotion gate: `manual` (default, an explicit approval is
    /// enough) or `evaluated` (an explicit approval is accepted only with an
    /// eligible immutable evaluation on record).
    pub experience_promotion: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:0".into(),
            database_url: "sqlite://s-code.db".into(),
            auth_mode: "development_token".into(),
            token: None,
            runner_token: None,
            team_grant_issuer: "s-code-control-plane".into(),
            team_grant_audience: "s-code-daemon".into(),
            team_grant_key_id: None,
            team_grant_public_key_base64: None,
            storage_encryption_key_id: None,
            storage_encryption_key_base64: None,
            team_config_key_id: None,
            team_config_public_key_base64: None,
            central_audit: CentralAuditConfig::default(),
            experience_mode: "off".into(),
            experience_promotion: "manual".into(),
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

/// The editing format exposed to a model; runtime protections are shared.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditingMode {
    #[default]
    Auto,
    Lines,
    Text,
    Patch,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    /// Enable read-only JavaScript tool orchestration.
    pub code_mode: bool,
    pub provider: String,
    pub base_url: Option<String>,
    pub credential_handle: Option<String>,
    pub endpoints: Vec<ModelEndpointConfig>,
    pub editing_mode: EditingMode,
    pub editing_overrides: BTreeMap<String, EditingMode>,
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
            code_mode: false,
            provider: "openai_compatible".into(),
            base_url: Some(DEFAULT_MODEL_API_BASE_URL.into()),
            credential_handle: None,
            endpoints: Vec::new(),
            editing_mode: EditingMode::Auto,
            editing_overrides: BTreeMap::new(),
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
        let file = environment
            .get("S_CODE_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                s_code_home_directory_from(|name| environment.get(name).map(OsString::from))
                    .ok()
                    .map(|home| home.join("config.toml"))
                    .filter(|path| path.is_file())
            });
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

        // Resolve authentication with the same env/CLI precedence used below before
        // reading local credentials. Team Grant must never inherit a local endpoint.
        let mut onboarding_auth = value.clone();
        if let Some(auth_mode) = self.environment.get("S_CODE_DAEMON_AUTH_MODE") {
            set_path(
                &mut onboarding_auth,
                "daemon.auth_mode",
                Value::String(auth_mode.clone()),
            );
        }
        for (path, override_value, _) in &self.overrides {
            set_path(&mut onboarding_auth, path, override_value.clone());
        }
        // The wizard is a local development setting; enterprise configuration is untouched.
        if value.get("profile").and_then(Value::as_str) != Some("production")
            && onboarding_auth
                .pointer("/daemon/auth_mode")
                .and_then(Value::as_str)
                == Some("development_token")
            && !onboarding::PROVIDER_ENVIRONMENT
                .iter()
                .any(|name| self.environment.contains_key(*name))
        {
            let provider_path = self
                .file
                .clone()
                .or_else(|| {
                    s_code_home_directory_from(|name| {
                        self.environment.get(name).map(OsString::from)
                    })
                    .ok()
                    .map(|home| home.join("config.toml"))
                })
                .map(|path| path.with_extension("provider-credentials.json"));
            if let Some(path) = provider_path
                && let Some(saved) = onboarding::read(&path).map_err(ConfigError::Invalid)?
            {
                // Only non-secret fields enter the effective configuration/provenance.
                let projection = serde_json::json!({ "model": {
                        "provider": saved.provider, "base_url": saved.base_url, "endpoints": []
                    }, "client": { "model": saved.model } });
                merge(
                    &mut value,
                    projection,
                    "",
                    SourceKind::File,
                    "local provider setup",
                    &mut provenance,
                )?;
            }
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
        if component == Component::Daemon
            && provenance
                .get("daemon.database_url")
                .is_some_and(|entry| entry.source == SourceKind::Default)
        {
            let state_directory = local_state_directory_from(|name| {
                self.environment
                    .get(name)
                    .map(OsString::from)
                    .or_else(|| std::env::var_os(name))
            })
            .map_err(ConfigError::Invalid)?;
            let database_url = sqlite_url(&state_directory.join("s-code.db"))?;
            set_path(
                &mut value,
                "daemon.database_url",
                Value::String(database_url),
            );
            provenance.insert(
                "daemon.database_url".into(),
                Provenance {
                    source: SourceKind::Default,
                    detail: "private user state directory".into(),
                },
            );
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
        env: "S_CODE_DAEMON_LISTEN",
        path: "daemon.listen",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_DATABASE_URL",
        path: "daemon.database_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_DAEMON_AUTH_MODE",
        path: "daemon.auth_mode",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TOKEN",
        path: "daemon.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_TOKEN",
        path: "daemon.runner_token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAM_GRANT_KEY_ID",
        path: "daemon.team_grant_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAM_GRANT_PUBLIC_KEY_BASE64",
        path: "daemon.team_grant_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_STORAGE_ENCRYPTION_KEY_ID",
        path: "daemon.storage_encryption_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_STORAGE_ENCRYPTION_KEY_BASE64",
        path: "daemon.storage_encryption_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAM_CONFIG_KEY_ID",
        path: "daemon.team_config_key_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAM_CONFIG_PUBLIC_KEY_BASE64",
        path: "daemon.team_config_public_key_base64",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_DAEMON_EXPERIENCE_MODE",
        path: "daemon.experience_mode",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_DAEMON_EXPERIENCE_PROMOTION",
        path: "daemon.experience_promotion",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_MODEL_PROVIDER",
        path: "model.provider",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_MODEL_BASE_URL",
        path: "model.base_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_MODEL_CREDENTIAL_HANDLE",
        path: "model.credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_MCP_ENABLED",
        path: "mcp.enabled",
        kind: EnvKind::Bool,
    },
    EnvMapping {
        env: "S_CODE_MCP_CONFIG",
        path: "mcp.config_path",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_DAEMON_URL",
        path: "runner.daemon_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_TOKEN",
        path: "runner.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_ID",
        path: "runner.worker_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_ALLOW_NETWORK",
        path: "runner.allow_network",
        kind: EnvKind::Bool,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_POLL_INTERVAL_MILLIS",
        path: "runner.poll_interval_millis",
        kind: EnvKind::U64,
    },
    EnvMapping {
        env: "S_CODE_RUNNER_LEASE_SECONDS",
        path: "runner.lease_seconds",
        kind: EnvKind::U64,
    },
    EnvMapping {
        env: "S_CODE_URL",
        path: "client.daemon_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TOKEN",
        path: "client.token",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_ORGANIZATION",
        path: "client.organization_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAM",
        path: "client.team_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_ACTOR",
        path: "client.actor_id",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_WORKSPACE",
        path: "client.workspace",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_MODEL",
        path: "client.model",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_THEME",
        path: "client.theme",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_KEYMAP",
        path: "client.keymap",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_STATUSLINE",
        path: "client.statusline",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_EDITOR",
        path: "client.editor",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_GITHUB_API_BASE",
        path: "connectors.github_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_GITHUB_REPOSITORY",
        path: "connectors.github_repository",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_GITHUB_CREDENTIAL_HANDLE",
        path: "connectors.github_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_CI_CREDENTIAL_HANDLE",
        path: "connectors.ci_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_JIRA_API_BASE",
        path: "connectors.jira_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_JIRA_PROJECT_KEY",
        path: "connectors.jira_project_key",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_JIRA_CREDENTIAL_HANDLE",
        path: "connectors.jira_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SERVICENOW_API_BASE",
        path: "connectors.servicenow_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SERVICENOW_TABLE",
        path: "connectors.servicenow_table",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SERVICENOW_CREDENTIAL_HANDLE",
        path: "connectors.servicenow_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SLACK_API_BASE",
        path: "connectors.slack_api_base",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SLACK_CREDENTIAL_HANDLE",
        path: "connectors.slack_credential_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_TEAMS_WEBHOOK_HANDLE",
        path: "connectors.teams_webhook_handle",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SPLUNK_HEC_BASE_URL",
        path: "connectors.splunk_hec_base_url",
        kind: EnvKind::String,
    },
    EnvMapping {
        env: "S_CODE_SPLUNK_CREDENTIAL_HANDLE",
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

fn validate(config: &RootConfig, component: Component) -> Result<(), ConfigError> {
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedVersion(config.schema_version));
    }
    match component {
        Component::Daemon => {
            let listen = config.daemon.listen.parse::<SocketAddr>().map_err(|_| {
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
            if !listen.ip().is_loopback() {
                return Err(ConfigError::Invalid(
                    "Community Local Web requires daemon.listen to use a loopback address".into(),
                ));
            }
            if !matches!(
                config.daemon.experience_mode.as_str(),
                "off" | "observe" | "verified"
            ) {
                return Err(ConfigError::Invalid(
                    "daemon.experience_mode must be off, observe or verified".into(),
                ));
            }
            if !matches!(
                config.daemon.experience_promotion.as_str(),
                "manual" | "evaluated"
            ) {
                return Err(ConfigError::Invalid(
                    "daemon.experience_promotion must be manual or evaluated".into(),
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
            if config.model.editing_overrides.len() > 128
                || config
                    .model
                    .editing_overrides
                    .keys()
                    .any(|key| key.trim().is_empty() || key.len() > 256)
            {
                return Err(ConfigError::Invalid("model.editing_overrides requires at most 128 nonempty model IDs of at most 256 bytes".into()));
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
            if let Some(base_url) = config.model.base_url.as_deref() {
                secure_model_base_url(base_url, "model.base_url")?;
            }
            for endpoint in &config.model.endpoints {
                secure_model_base_url(&endpoint.base_url, "model.endpoints[].base_url")?;
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
        "runner.token",
        "client.token",
    ] {
        if get_path(&serialized, path).is_some_and(|value| !value.is_null()) {
            require_secret_source(path, provenance)?;
        }
    }
    for path in ["daemon.database_url"] {
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
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::Invalid(format!(
            "{name} must not contain URL userinfo, query, or fragment"
        )));
    }
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!(
            "{name} must use HTTPS or loopback HTTP"
        )))
    }
}

fn secure_model_base_url(raw: &str, name: &str) -> Result<(), ConfigError> {
    secure_or_loopback_url(raw, name)
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
            if matches!(prefix, "secret_references" | "model.editing_overrides") {
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
        "runner.token",
        "client.token",
    ]
    .iter()
    .any(|field| get_path(value, field).is_some())
        || ["daemon.database_url"]
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
    fn editing_modes_load_from_real_toml_and_reject_unknown_modes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("editing.toml");
        std::fs::write(&path, "[model]\nediting_mode = \"text\"\n[model.editing_overrides]\n\"my-model\" = \"patch\"\n").unwrap();
        let effective = ConfigLoader::new()
            .with_file(&path)
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(effective.config.model.editing_mode, EditingMode::Text);
        assert_eq!(
            effective.config.model.editing_overrides["my-model"],
            EditingMode::Patch
        );
        std::fs::write(&path, "[model]\nediting_mode = \"typo\"\n").unwrap();
        assert!(
            ConfigLoader::new()
                .with_file(&path)
                .load(Component::Daemon)
                .is_err()
        );
    }

    #[test]
    fn local_daemon_connection_is_private_atomic_and_process_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("run").join("daemon.json");
        let connection = LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "generated-local-token".into(),
            "instance-0123456789".into(),
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
        let path = directory.path().join("run").join("daemon.json");
        let remote = LocalDaemonConnection::new(
            "https://daemon.example".into(),
            "generated-local-token".into(),
            "instance-0123456789".into(),
        );
        assert!(
            publish_local_daemon_connection_to(&remote, path.clone())
                .unwrap_err()
                .contains("loopback")
        );
        let local = LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "generated-local-token".into(),
            "instance-0123456789".into(),
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

    #[cfg(unix)]
    #[test]
    fn existing_runtime_override_permissions_are_rejected_without_mutation() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let runtime = directory.path().join("shared-runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        let connection = LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "generated-local-token".into(),
            "instance-0123456789".into(),
        );
        let error = publish_local_daemon_connection_to(&connection, runtime.join("daemon.json"))
            .unwrap_err();
        assert!(error.contains("0700"));
        assert_eq!(
            std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn daemon_default_database_uses_private_user_state_directory() {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("state");
        let effective = ConfigLoader::new()
            .with_environment([("S_CODE_STATE_DIR", state.as_os_str())])
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(
            effective.config.daemon.database_url,
            format!("sqlite://{}", state.join("s-code.db").display())
        );
        assert_eq!(
            effective.provenance("daemon.database_url").unwrap().detail,
            "private user state directory"
        );
    }

    #[test]
    fn development_token_daemon_rejects_non_loopback_listen() {
        let error = ConfigLoader::new()
            .with_environment([("S_CODE_DAEMON_LISTEN", "0.0.0.0:18788")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires daemon.listen to use a loopback")
        );
    }

    #[test]
    fn team_grant_daemon_also_rejects_non_loopback_listen() {
        let error = ConfigLoader::new()
            .with_environment([
                ("S_CODE_DAEMON_LISTEN", "0.0.0.0:18788"),
                ("S_CODE_DAEMON_AUTH_MODE", "team_grant"),
                ("S_CODE_TEAM_GRANT_KEY_ID", "key"),
                (
                    "S_CODE_TEAM_GRANT_PUBLIC_KEY_BASE64",
                    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                ),
            ])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("loopback"));
    }

    #[test]
    fn local_product_paths_require_absolute_overrides() {
        let error = ConfigLoader::new()
            .with_environment([("S_CODE_STATE_DIR", "relative")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("absolute path"));
        let runtime = BTreeMap::from([
            ("HOME", OsString::from("/private/home")),
            ("S_CODE_RUNTIME_DIR", OsString::from("relative/run")),
        ]);
        assert!(
            local_daemon_connection_path_from(|name| runtime.get(name).cloned())
                .unwrap_err()
                .contains("normalized absolute")
        );
        let state = BTreeMap::from([
            ("HOME", OsString::from("/private/home")),
            ("S_CODE_STATE_DIR", OsString::from("/private/home/../state")),
        ]);
        assert!(
            local_state_directory_from(|name| state.get(name).cloned())
                .unwrap_err()
                .contains("normalized absolute")
        );
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
            .with_environment([("S_CODE_RUNNER_TOKEN", "env-token")])
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
                ("S_CODE_TOKEN", "super-secret"),
                ("S_CODE_URL", "http://127.0.0.1:4096"),
            ])
            .load(Component::Cli)
            .unwrap();
        let text = serde_json::to_string(&effective.redacted_json()).unwrap();
        assert!(!text.contains("super-secret"));
        assert_eq!(effective.redacted_json()["client"]["token"], "[REDACTED]");
        assert!(effective.redacted_json()["runner"]["token"].is_null());
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
            .with_environment([("S_CODE_TOKEN", "x"), ("S_CODE_URL", "http://example.com")])
            .load(Component::Cli)
            .unwrap_err();
        assert!(error.to_string().contains("HTTPS"));
        for endpoint in [
            "https://user:password@example.com/v1",
            "https://example.com/v1?token=secret",
            "https://example.com/v1#fragment",
        ] {
            let error = ConfigLoader::new()
                .with_environment([("S_CODE_TOKEN", "x"), ("S_CODE_URL", endpoint)])
                .load(Component::Cli)
                .unwrap_err();
            assert!(
                error.to_string().contains("userinfo, query, or fragment"),
                "accepted or misclassified {endpoint}: {error}"
            );
        }
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
                ("S_CODE_TOKEN", "injected-token"),
                ("S_CODE_DATABASE_URL", "sqlite://injected.db"),
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
    fn experience_promotion_defaults_to_manual_and_rejects_unknown_values() {
        let effective = ConfigLoader::new().load(Component::Daemon).unwrap();
        assert_eq!(effective.config.daemon.experience_promotion, "manual");
        let effective = ConfigLoader::new()
            .with_environment([("S_CODE_DAEMON_EXPERIENCE_PROMOTION", "evaluated")])
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(effective.config.daemon.experience_promotion, "evaluated");
        let error = ConfigLoader::new()
            .with_environment([("S_CODE_DAEMON_EXPERIENCE_PROMOTION", "automatic")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("manual or evaluated"));
    }

    #[test]
    fn experience_mode_defaults_to_off_and_rejects_unknown_values() {
        let effective = ConfigLoader::new().load(Component::Daemon).unwrap();
        assert_eq!(effective.config.daemon.experience_mode, "off");
        for mode in ["observe", "verified"] {
            let effective = ConfigLoader::new()
                .with_environment([("S_CODE_DAEMON_EXPERIENCE_MODE", mode)])
                .load(Component::Daemon)
                .unwrap();
            assert_eq!(effective.config.daemon.experience_mode, mode);
        }
        let error = ConfigLoader::new()
            .with_environment([("S_CODE_DAEMON_EXPERIENCE_MODE", "autonomous")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("off, observe or verified"));
    }

    #[test]
    fn model_provider_selection_is_explicit_and_atomic() {
        let effective = ConfigLoader::new()
            .with_environment([
                ("S_CODE_MODEL_PROVIDER", "anthropic"),
                ("S_CODE_MODEL_BASE_URL", "https://api.anthropic.com/v1"),
                ("S_CODE_MODEL_CREDENTIAL_HANDLE", "ANTHROPIC_API_KEY"),
            ])
            .load(Component::Daemon)
            .unwrap();
        assert_eq!(effective.config.model.provider, "anthropic");

        let error = ConfigLoader::new()
            .with_environment([("S_CODE_MODEL_PROVIDER", "unknown")])
            .load(Component::Daemon)
            .unwrap_err();
        assert!(error.to_string().contains("openai_compatible"));

        ConfigLoader::new()
            .with_environment([(
                "S_CODE_MODEL_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta",
            )])
            .load(Component::Daemon)
            .unwrap();

        let configured = ConfigLoader::new()
            .with_environment([("S_CODE_MODEL_CREDENTIAL_HANDLE", "GEMINI_API_KEY")])
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
    fn development_model_endpoints_reject_credential_exfiltration_urls() {
        for unsafe_url in [
            "http://models.example/v1",
            "https://user:password@models.example/v1",
            "https://models.example/v1?forward=credential",
            "https://models.example/v1#fragment",
        ] {
            let mut config = RootConfig {
                profile: Profile::Development,
                ..RootConfig::default()
            };
            config.model.base_url = Some(unsafe_url.into());
            let error = validate(&config, Component::Daemon).unwrap_err();
            assert!(
                error.to_string().contains("model.base_url"),
                "unexpected validation result for {unsafe_url}: {error}"
            );
        }

        let mut routed = RootConfig {
            profile: Profile::Development,
            ..RootConfig::default()
        };
        routed.model.base_url = None;
        routed.model.credential_handle = None;
        routed.model.endpoints = vec![ModelEndpointConfig {
            id: "unsafe".into(),
            provider: "openai_compatible".into(),
            provider_model: "model".into(),
            base_url: "http://models.example/v1".into(),
            credential_handle: Some("MODEL_API_KEY".into()),
        }];
        let error = validate(&routed, Component::Daemon).unwrap_err();
        assert!(error.to_string().contains("model.endpoints[].base_url"));

        routed.model.endpoints[0].base_url = "http://127.0.0.1:18787/v1".into();
        validate(&routed, Component::Daemon).unwrap();
    }

    #[test]
    fn cli_interface_settings_are_typed_validated_and_environment_configurable() {
        let configured = ConfigLoader::new()
            .with_environment([
                ("S_CODE_THEME", "light"),
                ("S_CODE_KEYMAP", "vim"),
                ("S_CODE_STATUSLINE", "compact"),
                ("S_CODE_EDITOR", "code --wait"),
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
            ("S_CODE_THEME", "sepia", "client.theme"),
            ("S_CODE_KEYMAP", "random", "client.keymap"),
            ("S_CODE_STATUSLINE", "verbose", "client.statusline"),
        ] {
            let error = ConfigLoader::new()
                .with_environment([(environment, value)])
                .load(Component::Cli)
                .unwrap_err();
            assert!(error.to_string().contains(expected));
        }
    }
}
