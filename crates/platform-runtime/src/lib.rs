pub mod sensitive_paths;

use async_trait::async_trait;
use s_code_protocol::Capability;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use thiserror::Error;
use tokio::io::AsyncReadExt;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("capability unavailable: {0}")]
    CapabilityUnavailable(String),
    #[error("invalid workspace boundary: {0}")]
    InvalidBoundary(String),
    #[error("execution failed: {0}")]
    Execution(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd_uri: String,
    #[serde(default)]
    pub environment_handles: BTreeMap<String, String>,
    pub timeout: Duration,
    pub network_enabled: bool,
    #[serde(default)]
    pub browser_compatible: bool,
    #[serde(default)]
    pub readable_root_uris: Vec<String>,
    #[serde(default)]
    pub writable_root_uris: Vec<String>,
    #[serde(default)]
    pub denied_read_uris: Vec<String>,
    #[serde(default = "default_output_limit")]
    pub output_limit_bytes: usize,
    /// Already-open workspace directory used to close pathname substitution
    /// between authorization and process spawn. This is intentionally local
    /// process state and is never serialized.
    #[cfg(unix)]
    #[serde(skip)]
    pub pinned_cwd: Option<Arc<std::fs::File>>,
}

fn default_output_limit() -> usize {
    1024 * 1024
}

#[cfg(unix)]
fn kill_process_group(process_id: u32) {
    // Every sandbox command starts in its own process group. Signalling the
    // negative group id closes the fork-after-snapshot race inherent in
    // walking the process tree with pgrep.
    let _ = std::process::Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{process_id}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(unix)]
struct ProcessGroupGuard {
    process_id: Option<u32>,
}

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(process_id) = self.process_id.take() {
            kill_process_group(process_id);
        }
    }
}

async fn drain_limited<R>(
    mut reader: R,
    remaining: Arc<AtomicUsize>,
) -> std::io::Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let available = remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(read))
            })
            .unwrap_or(0);
        let keep = available.min(read);
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    Ok((retained, truncated))
}

/// Return a minimal executable search path for a cleared host-process
/// environment. The explicitly selected program's directory comes first so
/// `/usr/bin/env node` launchers can find a sibling interpreter without
/// inheriting the daemon's potentially credentialed or workspace-controlled
/// PATH.
pub fn sanitized_host_extension_path(program: &str) -> OsString {
    let mut directories = Vec::new();
    if let Some(parent) = Path::new(program)
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        directories.push(parent.to_path_buf());
    }
    #[cfg(unix)]
    for directory in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let directory = PathBuf::from(directory);
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    #[cfg(windows)]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        for directory in [
            PathBuf::from(&system_root).join("System32"),
            PathBuf::from(system_root),
        ] {
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    std::env::join_paths(directories).unwrap_or_default()
}

/// Resolve an interpreter or helper with exactly the PATH that will be passed
/// to the host extension. Approval hashing and execution must call the same
/// resolver so a different ambient daemon PATH cannot select a different file.
pub fn resolve_sanitized_host_executable(program: &str, name: &str) -> Option<PathBuf> {
    std::env::split_paths(&sanitized_host_extension_path(program))
        .map(|directory| directory.join(name))
        .find(|candidate| executable_file(candidate))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

#[async_trait]
pub trait PlatformRuntime: Send + Sync {
    fn capabilities(&self) -> Vec<Capability>;
    fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError>;
    async fn execute(&self, spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError>;
    fn supports_host_execution(&self) -> bool {
        false
    }
    /// Trusted host-only entry point. Never selected by serialized process/tool arguments.
    async fn execute_host(&self, _spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable("host execution".into()))
    }
    async fn cancel_process_tree(&self, process_id: &str) -> Result<(), RuntimeError>;
}

pub struct UnsupportedRuntime {
    pub platform: String,
}

pub struct NativeRuntime;

impl NativeRuntime {
    fn uri_path(uri: &str) -> Result<PathBuf, RuntimeError> {
        let url = url::Url::parse(uri).map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
        if url.scheme() != "file" {
            return Err(RuntimeError::InvalidBoundary(
                "only file:// URIs are supported".into(),
            ));
        }
        url.to_file_path()
            .map_err(|_| RuntimeError::InvalidBoundary(uri.into()))
    }

    fn command(spec: &ProcessSpec) -> Result<tokio::process::Command, RuntimeError> {
        #[cfg(target_os = "macos")]
        {
            let mut profile = String::from(
                "(version 1)(deny default)(import \"system.sb\")(allow process*)(allow signal (target children))(allow sysctl-read)(allow mach-lookup)(allow file-read-metadata)",
            );
            if spec.browser_compatible {
                profile.push_str("(allow ipc-posix-shm)(allow ipc-posix-sem)");
            }
            for uri in &spec.readable_root_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
                profile.push_str(&format!(
                    "(allow file-read* (subpath {}))",
                    seatbelt_string(&path)
                ));
            }
            for system in [
                "/System",
                "/usr/bin",
                "/usr/lib",
                "/usr/share",
                "/bin",
                "/sbin",
                "/dev",
            ] {
                profile.push_str(&format!("(allow file-read* (subpath \"{system}\"))"));
            }
            // System OpenSSL reads this fixed, root-owned configuration path
            // even for offline compiler/linker invocations. Keep the grant to
            // the single file instead of exposing all of /private/etc.
            let openssl_configuration = std::path::Path::new("/private/etc/ssl/openssl.cnf");
            if openssl_configuration.is_file() {
                profile.push_str(&format!(
                    "(allow file-read* (literal {}))",
                    seatbelt_string(openssl_configuration)
                ));
            }
            if let Some(xcode_read_root) = macos_xcode_read_root() {
                profile.push_str(&format!(
                    "(allow file-read* (subpath {}))",
                    seatbelt_string(&xcode_read_root)
                ));
            }
            if let Some(command_line_tools_root) = macos_command_line_tools_read_root() {
                profile.push_str(&format!(
                    "(allow file-read* (subpath {}))",
                    seatbelt_string(&command_line_tools_root)
                ));
            }
            append_macos_dynamic_library_rules(
                &mut profile,
                macos_dynamic_libraries(std::path::Path::new(&spec.program)),
            );
            for uri in &spec.writable_root_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
                profile.push_str(&format!(
                    "(allow file-read* file-write* (subpath {}))",
                    seatbelt_string(&path)
                ));
                let git_metadata = path.join(".git");
                if git_metadata.exists() {
                    profile.push_str(&format!(
                        "(deny file-write* (literal {}) (subpath {}))",
                        seatbelt_string(&git_metadata),
                        seatbelt_string(&git_metadata)
                    ));
                }
            }
            for uri in &spec.denied_read_uris {
                let path = Self::uri_path(uri)?;
                profile.push_str(&format!(
                    "(deny file-read* file-write* (literal {}) (subpath {}))",
                    seatbelt_string(&path),
                    seatbelt_string(&path)
                ));
                // A writable command must not rename a sensitive file's
                // ancestor and then read the same bytes under a new lexical
                // path. Protect every descendant anchor while leaving writes
                // to unrelated siblings available.
                for writable_uri in &spec.writable_root_uris {
                    let writable_root = Self::uri_path(writable_uri)?
                        .canonicalize()
                        .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
                    let mut ancestor = path.parent();
                    while let Some(candidate) = ancestor {
                        if candidate == writable_root || !candidate.starts_with(&writable_root) {
                            break;
                        }
                        profile.push_str(&format!(
                            "(deny file-write-unlink (literal {}))",
                            seatbelt_string(candidate)
                        ));
                        ancestor = candidate.parent();
                    }
                }
            }
            if spec.network_enabled {
                profile.push_str("(allow network*)");
            }
            let mut command = tokio::process::Command::new("/usr/bin/sandbox-exec");
            command
                .arg("-p")
                .arg(profile)
                .arg(&spec.program)
                .args(&spec.args);
            Ok(command)
        }
        #[cfg(target_os = "linux")]
        {
            let mut command = tokio::process::Command::new("bwrap");
            command.args([
                "--die-with-parent",
                "--new-session",
                "--unshare-all",
                "--tmpfs",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
            ]);
            for system in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
                if std::path::Path::new(system).exists() {
                    command.arg("--ro-bind").arg(system).arg(system);
                }
            }
            command
                .arg("--dir")
                .arg("/etc")
                .arg("--dir")
                .arg("/etc/ssl");
            for system in [
                "/etc/ld.so.cache",
                "/etc/alternatives",
                "/etc/ssl/certs",
                "/etc/ca-certificates",
            ] {
                if std::path::Path::new(system).exists() {
                    command.arg("--ro-bind").arg(system).arg(system);
                }
            }
            if spec.network_enabled {
                command.arg("--share-net");
                for system in ["/etc/hosts", "/etc/resolv.conf", "/etc/nsswitch.conf"] {
                    if std::path::Path::new(system).is_file() {
                        command.arg("--ro-bind").arg(system).arg(system);
                    }
                }
            }
            for uri in &spec.readable_root_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
            let mut writable_roots = Vec::new();
            let mut git_metadata_roots = Vec::new();
            for uri in &spec.writable_root_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
                command.arg("--bind").arg(&path).arg(&path);
                let git_metadata = path.join(".git");
                if git_metadata.exists() {
                    git_metadata_roots.push(git_metadata);
                }
                writable_roots.push(path);
            }
            let mut denied_paths = Vec::new();
            for uri in &spec.denied_read_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
                denied_paths.push(path);
            }
            // An exact overmount hides a sensitive file, but a command could
            // otherwise rename one of its writable ancestors and reach the
            // original inode at a new path. A self-bind makes each ancestor a
            // mount point (and therefore unrenameable) without removing write
            // access to its non-sensitive children.
            for anchor in sensitive_path_anchors(&denied_paths, &writable_roots) {
                command.arg("--bind").arg(&anchor).arg(&anchor);
            }
            // Apply the read-only Git mount after writable ancestor anchors so
            // an anchored .git directory never becomes writable again.
            for git_metadata in git_metadata_roots {
                command
                    .arg("--ro-bind")
                    .arg(&git_metadata)
                    .arg(&git_metadata);
            }
            for path in denied_paths {
                if path.is_dir() {
                    command.arg("--tmpfs").arg(&path);
                } else {
                    command.arg("--ro-bind").arg("/dev/null").arg(&path);
                }
            }
            command.arg("--").arg(&spec.program).args(&spec.args);
            Ok(command)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = spec;
            Err(RuntimeError::CapabilityUnavailable(
                "native sandbox backend".into(),
            ))
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn sensitive_path_anchors(
    denied_paths: &[PathBuf],
    writable_roots: &[PathBuf],
) -> std::collections::BTreeSet<PathBuf> {
    let mut anchors = std::collections::BTreeSet::new();
    for denied_path in denied_paths {
        for writable_root in writable_roots {
            let mut ancestor = denied_path.parent();
            while let Some(candidate) = ancestor {
                if candidate == writable_root || !candidate.starts_with(writable_root) {
                    break;
                }
                anchors.insert(candidate.to_path_buf());
                ancestor = candidate.parent();
            }
        }
    }
    anchors
}

#[cfg(target_os = "macos")]
fn append_macos_dynamic_library_rules(
    profile: &mut String,
    libraries: impl IntoIterator<Item = PathBuf>,
) {
    for library in libraries {
        profile.push_str(&format!(
            "(allow file-read* (literal {}))",
            seatbelt_string(&library)
        ));
        if let Ok(canonical) = library.canonicalize()
            && canonical != library
        {
            profile.push_str(&format!(
                "(allow file-read* (literal {}))",
                seatbelt_string(&canonical)
            ));
        }
    }
}

#[cfg(target_os = "macos")]
fn trusted_macos_xcode_read_root(developer_directory: &std::path::Path) -> Option<PathBuf> {
    if !developer_directory.starts_with("/Applications")
        || !developer_directory.ends_with("Contents/Developer")
        || !developer_directory.is_absolute()
    {
        return None;
    }
    let contents = developer_directory.parent()?;
    let application = contents.parent()?;
    (contents.file_name().and_then(|name| name.to_str()) == Some("Contents")
        && application
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("app"))
    .then(|| application.to_path_buf())
}

#[cfg(target_os = "macos")]
fn macos_xcode_read_root() -> Option<PathBuf> {
    let output = std::process::Command::new("/usr/bin/xcode-select")
        .arg("--print-path")
        .env_remove("DEVELOPER_DIR")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    let canonical = path.canonicalize().ok()?;
    trusted_macos_xcode_read_root(&canonical)
}

#[cfg(target_os = "macos")]
fn macos_command_line_tools_read_root() -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let root = PathBuf::from("/Library/Developer/CommandLineTools");
    let selected = std::process::Command::new("/usr/bin/xcode-select")
        .arg("--print-path")
        .env_remove("DEVELOPER_DIR")
        .output()
        .ok()?;
    if !selected.status.success()
        || PathBuf::from(String::from_utf8(selected.stdout).ok()?.trim())
            .canonicalize()
            .ok()?
            != root
    {
        return None;
    }
    let metadata = root.metadata().ok()?;
    (metadata.uid() == 0 && metadata.permissions().mode() & 0o022 == 0).then_some(root)
}

#[cfg(target_os = "macos")]
fn macos_dynamic_libraries(program: &std::path::Path) -> Vec<PathBuf> {
    let executable = program
        .canonicalize()
        .unwrap_or_else(|_| program.to_path_buf());
    let mut pending = vec![executable.clone()];
    let mut inspected = std::collections::BTreeSet::new();
    let mut libraries = std::collections::BTreeSet::new();
    while let Some(loader) = pending.pop() {
        if inspected.len() >= 256 || !inspected.insert(loader.clone()) {
            continue;
        }
        let Ok(output) = std::process::Command::new("/usr/bin/otool")
            .arg("-L")
            .arg(&loader)
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let rpaths = macos_loader_rpaths(&loader, &executable);
        for reference in String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
        {
            let Some(path) = resolve_macos_library(reference, &loader, &executable, &rpaths) else {
                continue;
            };
            if path.starts_with("/System") || path.starts_with("/usr/lib") {
                continue;
            }
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            if !canonical.is_file() || canonical == executable {
                continue;
            }
            libraries.insert(path);
            if !inspected.contains(&canonical) {
                pending.push(canonical);
            }
        }
    }
    libraries.into_iter().collect()
}

#[cfg(target_os = "macos")]
fn macos_loader_rpaths(loader: &std::path::Path, executable: &std::path::Path) -> Vec<PathBuf> {
    let Ok(output) = std::process::Command::new("/usr/bin/otool")
        .arg("-l")
        .arg(loader)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut expects_path = false;
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line == "cmd LC_RPATH" {
            expects_path = true;
            continue;
        }
        if expects_path && line.starts_with("path ") {
            if let Some(reference) = line
                .strip_prefix("path ")
                .and_then(|value| value.split_whitespace().next())
                && let Some(path) = resolve_macos_loader_path(reference, loader, executable)
            {
                paths.push(path);
            }
            expects_path = false;
        }
    }
    paths
}

#[cfg(target_os = "macos")]
fn resolve_macos_loader_path(
    reference: &str,
    loader: &std::path::Path,
    executable: &std::path::Path,
) -> Option<PathBuf> {
    if let Some(relative) = reference.strip_prefix("@loader_path/") {
        return loader.parent().map(|parent| parent.join(relative));
    }
    if reference == "@loader_path" {
        return loader.parent().map(PathBuf::from);
    }
    if let Some(relative) = reference.strip_prefix("@executable_path/") {
        return executable.parent().map(|parent| parent.join(relative));
    }
    if reference == "@executable_path" {
        return executable.parent().map(PathBuf::from);
    }
    let path = PathBuf::from(reference);
    path.is_absolute().then_some(path)
}

#[cfg(target_os = "macos")]
fn resolve_macos_library(
    reference: &str,
    loader: &std::path::Path,
    executable: &std::path::Path,
    rpaths: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(relative) = reference.strip_prefix("@rpath/") {
        return rpaths
            .iter()
            .map(|root| root.join(relative))
            .find(|path| path.is_file());
    }
    resolve_macos_loader_path(reference, loader, executable).filter(|path| path.is_file())
}

#[cfg(target_os = "macos")]
fn seatbelt_string(path: &std::path::Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[async_trait]
impl PlatformRuntime for NativeRuntime {
    fn capabilities(&self) -> Vec<Capability> {
        let id = if cfg!(target_os = "macos") {
            "sandbox.seatbelt"
        } else if cfg!(target_os = "linux") {
            "sandbox.bubblewrap"
        } else {
            "sandbox.unsupported"
        };
        vec![Capability {
            id: id.into(),
            version: "1".into(),
            maturity: s_code_protocol::CapabilityMaturity::Preview,
            enabled: cfg!(any(target_os = "macos", target_os = "linux")),
            attributes: Default::default(),
        }]
    }

    fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError> {
        let path = Self::uri_path(uri)?;
        path.canonicalize()
            .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))
    }

    async fn execute(&self, spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
        Self::execute_native(spec, false).await
    }

    fn supports_host_execution(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "linux"))
    }

    async fn execute_host(&self, spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
        Self::execute_native(spec, true).await
    }

    async fn cancel_process_tree(&self, _process_id: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable(
            "process registry not initialized".into(),
        ))
    }
}

impl NativeRuntime {
    async fn execute_native(
        mut spec: ProcessSpec,
        host: bool,
    ) -> Result<ProcessOutput, RuntimeError> {
        let deadline = tokio::time::Instant::now() + spec.timeout;
        let cwd = Self::uri_path(&spec.cwd_uri)?
            .canonicalize()
            .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
        #[cfg(unix)]
        if let Some(directory) = &spec.pinned_cwd {
            use std::os::unix::fs::MetadataExt;

            let opened = directory
                .metadata()
                .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
            let named = cwd
                .metadata()
                .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))?;
            if opened.dev() != named.dev() || opened.ino() != named.ino() {
                return Err(RuntimeError::InvalidBoundary(
                    "workspace path no longer names the authorized directory".into(),
                ));
            }
        }
        let sandbox_temp = tempfile::Builder::new()
            .prefix("s-code-sandbox.")
            .tempdir()
            .map_err(|error| RuntimeError::Execution(error.to_string()))?;
        let sandbox_temp_uri = url::Url::from_directory_path(sandbox_temp.path())
            .map_err(|_| RuntimeError::Execution("sandbox temp path is not absolute".into()))?
            .to_string();
        spec.writable_root_uris.push(sandbox_temp_uri);
        let mut command = if host {
            if !cfg!(any(target_os = "macos", target_os = "linux")) {
                return Err(RuntimeError::CapabilityUnavailable("host execution".into()));
            }
            if !spec.denied_read_uris.is_empty() {
                return Err(RuntimeError::InvalidBoundary(
                    "host execution cannot enforce file denies".into(),
                ));
            }
            let mut command = tokio::process::Command::new(&spec.program);
            command.args(&spec.args);
            command
        } else {
            Self::command(&spec)?
        };
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(unix)]
        if let Some(directory) = spec.pinned_cwd.clone() {
            use std::os::unix::{fs::MetadataExt, process::CommandExt};

            let named_path = cwd.clone();
            command.current_dir("/");
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    let opened = directory.metadata()?;
                    let named = std::fs::metadata(&named_path)?;
                    if opened.dev() != named.dev() || opened.ino() != named.ino() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "workspace path no longer names the authorized directory",
                        ));
                    }
                    rustix::process::fchdir(&*directory).map_err(std::io::Error::from)
                });
            }
        } else {
            command.current_dir(&cwd);
        }
        #[cfg(not(unix))]
        command.current_dir(&cwd);
        command
            .env_clear()
            .env("PATH", sanitized_host_extension_path(&spec.program))
            .env("TMPDIR", sandbox_temp.path())
            .kill_on_drop(true);
        if std::path::Path::new(&spec.program)
            .file_name()
            .is_some_and(|name| name == "node")
        {
            command.env("OPENSSL_CONF", "/dev/null");
        }
        for (name, value) in &spec.environment_handles {
            command.env(name, value);
        }
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| RuntimeError::Execution(error.to_string()))?;
        #[cfg(unix)]
        let process_id = child.id();
        #[cfg(unix)]
        let mut process_group = ProcessGroupGuard { process_id };
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Execution("failed to capture stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Execution("failed to capture stderr".into()))?;
        let remaining = Arc::new(AtomicUsize::new(spec.output_limit_bytes));
        let mut stdout_task = tokio::spawn(drain_limited(stdout, remaining.clone()));
        let mut stderr_task = tokio::spawn(drain_limited(stderr, remaining));
        let status = match tokio::time::timeout_at(deadline, child.wait()).await {
            Ok(status) => status.map_err(|error| RuntimeError::Execution(error.to_string()))?,
            Err(_) => {
                #[cfg(unix)]
                if let Some(process_id) = process_id {
                    kill_process_group(process_id);
                }
                let _ = child.start_kill();
                let _ = child.wait().await;
                stdout_task.abort();
                stderr_task.abort();
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(RuntimeError::Execution("process timed out".into()));
            }
        };
        #[cfg(unix)]
        if let Some(process_id) = process_id {
            kill_process_group(process_id);
        }
        let drains = tokio::time::timeout_at(deadline, async {
            let stdout = (&mut stdout_task)
                .await
                .map_err(|error| RuntimeError::Execution(error.to_string()))?
                .map_err(|error| RuntimeError::Execution(error.to_string()))?;
            let stderr = (&mut stderr_task)
                .await
                .map_err(|error| RuntimeError::Execution(error.to_string()))?
                .map_err(|error| RuntimeError::Execution(error.to_string()))?;
            Ok::<_, RuntimeError>((stdout, stderr))
        })
        .await;
        let ((stdout, stdout_cut), (stderr, stderr_cut)) = match drains {
            Ok(result) => result?,
            Err(_) => {
                stdout_task.abort();
                stderr_task.abort();
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(RuntimeError::Execution(
                    "process output drain timed out".into(),
                ));
            }
        };
        #[cfg(unix)]
        {
            process_group.process_id = None;
        }
        Ok(ProcessOutput {
            exit_code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            truncated: stdout_cut || stderr_cut,
        })
    }
}

#[async_trait]
impl PlatformRuntime for UnsupportedRuntime {
    fn capabilities(&self) -> Vec<Capability> {
        vec![]
    }
    fn canonicalize_workspace(&self, _: &str) -> Result<PathBuf, RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable(self.platform.clone()))
    }
    async fn execute(&self, _: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable(self.platform.clone()))
    }
    async fn cancel_process_tree(&self, _: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable(self.platform.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    const UNIX_SANDBOX_TEST_TIMEOUT: Duration = Duration::from_secs(15);

    #[test]
    fn sensitive_ancestors_are_anchored_without_anchoring_the_workspace_root() {
        let workspace = PathBuf::from("/workspace");
        let denied = vec![
            workspace.join("app/config/.env"),
            workspace.join("app/secrets/token"),
            workspace.join(".env"),
        ];

        assert_eq!(
            sensitive_path_anchors(&denied, std::slice::from_ref(&workspace)),
            std::collections::BTreeSet::from([
                workspace.join("app"),
                workspace.join("app/config"),
                workspace.join("app/secrets"),
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn sanitized_host_extension_path_includes_the_program_directory() {
        assert_eq!(
            sanitized_host_extension_path("/opt/tools/bin/npm").to_string_lossy(),
            "/opt/tools/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(
            sanitized_host_extension_path("/usr/bin/python3").to_string_lossy(),
            "/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_application_xcode_roots_are_trusted_for_runtime_reads() {
        assert_eq!(
            trusted_macos_xcode_read_root(std::path::Path::new(
                "/Applications/Xcode_26.6.app/Contents/Developer"
            )),
            Some(PathBuf::from("/Applications/Xcode_26.6.app"))
        );
        assert_eq!(
            trusted_macos_xcode_read_root(std::path::Path::new(
                "/Users/developer/Xcode.app/Contents/Developer"
            )),
            None
        );
        assert_eq!(
            trusted_macos_xcode_read_root(std::path::Path::new(
                "/Applications/Xcode_26.6.app/Contents"
            )),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dynamic_library_rules_never_expose_sibling_files() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("private-libraries");
        std::fs::create_dir(&private).unwrap();
        let target = private.join("libtarget.dylib");
        let link = private.join("libalias.dylib");
        std::fs::write(&target, b"fixture").unwrap();
        symlink(&target, &link).unwrap();

        let mut profile = String::new();
        append_macos_dynamic_library_rules(&mut profile, [link.clone()]);

        assert!(profile.contains(&format!("(literal {})", seatbelt_string(&link))));
        assert!(profile.contains(&format!(
            "(literal {})",
            seatbelt_string(&target.canonicalize().unwrap())
        )));
        assert!(!profile.contains(&format!("(subpath {})", seatbelt_string(&private))));
    }

    #[tokio::test]
    async fn explicitly_unsupported_runtime_fails_closed() {
        let runtime = UnsupportedRuntime {
            platform: "fixture-unsupported".into(),
        };
        assert!(matches!(
            runtime.canonicalize_workspace("file:///C:/src"),
            Err(RuntimeError::CapabilityUnavailable(_))
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn sandbox_temp_is_writable_without_expanding_workspace_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "touch \"$TMPDIR/probe\" && if touch workspace-denied 2>/dev/null; then exit 9; fi"
                        .into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri],
                writable_root_uris: vec![],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(!workspace.path().join("workspace-denied").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn command_output_is_drained_while_retained_memory_stays_bounded() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let limit = 16 * 1024;
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/perl".into(),
                args: vec![
                    "-e".into(),
                    "for (1..128) { print 'x' x 65536; print STDERR 'x' x 65536; }".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(10),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: limit,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(output.truncated);
        assert_eq!(output.stdout.len() + output.stderr.len(), limit);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn windows_native_runtime_is_explicitly_unsupported_and_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let capability = NativeRuntime
            .capabilities()
            .into_iter()
            .find(|capability| capability.id == "sandbox.unsupported")
            .expect("Windows must advertise the unavailable native sandbox");
        assert!(!capability.enabled);
        assert!(matches!(
            NativeRuntime
                .execute(ProcessSpec {
                    program: "cmd.exe".into(),
                    args: vec!["/c".into(), "exit".into(), "0".into()],
                    cwd_uri: workspace_uri.clone(),
                    environment_handles: Default::default(),
                    timeout: Duration::from_secs(5),
                    network_enabled: false,
                    browser_compatible: false,
                    readable_root_uris: vec![workspace_uri],
                    writable_root_uris: vec![],
                    denied_read_uris: vec![],
                    output_limit_bytes: 4096,
                    #[cfg(unix)]
                    pinned_cwd: None,
                })
                .await,
            Err(RuntimeError::CapabilityUnavailable(_))
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn native_read_only_profile_denies_workspace_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo denied > denied.txt".into()],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri],
                writable_root_uris: vec![],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(!workspace.path().join("denied.txt").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn writable_workspace_profile_keeps_git_metadata_read_only() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".git")).unwrap();
        std::fs::write(workspace.path().join(".git/config"), "[core]\n").unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "echo allowed > source.txt; echo malicious >> .git/config".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(workspace.path().join("source.txt").exists());
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(".git/config")).unwrap(),
            "[core]\n"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn timeout_kills_the_command_process_group() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let delayed_side_effect = workspace.path().join("child-survived");
        let result = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "(sleep 1; printf survived > child-survived) & wait".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_millis(250),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await;
        assert!(
            matches!(result, Err(RuntimeError::Execution(message)) if message == "process timed out")
        );
        // Linux bubblewrap uses a PID namespace, so a PID written from inside
        // the sandbox cannot be queried safely from the host. A delayed,
        // workspace-visible side effect proves the child did not survive the
        // group kill without confusing a namespace PID for an unrelated host
        // process.
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(
            !delayed_side_effect.exists(),
            "timed-out child completed a delayed side effect"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn successful_parent_exit_does_not_leave_inherited_output_pipes_open() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            NativeRuntime.execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 30 & printf complete".into()],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(1),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            }),
        )
        .await
        .expect("runtime did not reclaim inherited output pipes")
        .unwrap();
        assert_eq!(result.exit_code, Some(0), "sandbox output: {result:?}");
        assert_eq!(result.stdout, "complete");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandboxed_process_group_signals_cannot_kill_the_harness() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/perl".into(),
                args: vec![
                    "-MPOSIX".into(),
                    "-e".into(),
                    "kill 9, -POSIX::getpgrp();".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, None, "sandbox output: {output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandboxed_programs_may_create_their_own_child_session() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/perl".into(),
                args: vec![
                    "-MPOSIX".into(),
                    "-e".into(),
                    "defined(my $pid = fork) or die 'fork'; if (!$pid) { POSIX::setsid() >= 0 or die 'setsid'; exec '/usr/bin/true'; } waitpid($pid, 0); exit(($? >> 8) || ($? & 127));".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn sandboxed_program_can_reap_a_timed_out_grandchild() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/perl".into(),
                args: vec![
                    "-e".into(),
                    "defined(my $pid = fork) or die 'fork'; if (!$pid) { sleep 5; exit 0; } select undef, undef, undef, 0.1; kill 9, $pid; waitpid($pid, 0); print qq(reaped\\n);".into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert_eq!(output.stdout.trim(), "reaped", "sandbox output: {output:?}");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn sandbox_denies_external_reads_in_every_profile() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let secret = external.path().join("host-secret.txt");
        std::fs::write(&secret, "host-secret-canary-38e012f8").unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        for browser_compatible in [false, true] {
            let output = NativeRuntime
                .execute(ProcessSpec {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        format!("cat '{}' 2>/dev/null", secret.display()),
                    ],
                    cwd_uri: workspace_uri.clone(),
                    environment_handles: Default::default(),
                    timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                    network_enabled: false,
                    browser_compatible,
                    readable_root_uris: vec![workspace_uri.clone()],
                    writable_root_uris: vec![workspace_uri.clone()],
                    denied_read_uris: vec![],
                    output_limit_bytes: 4096,
                    #[cfg(unix)]
                    pinned_cwd: None,
                })
                .await
                .unwrap();
            assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
            assert!(!output.stdout.contains("host-secret-canary-38e012f8"));
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn sandbox_does_not_expose_broad_system_configuration_roots() {
        let protected = if cfg!(target_os = "macos") {
            std::path::Path::new("/Library/Keychains")
        } else {
            std::path::Path::new("/etc/hostname")
        };
        if !protected.exists() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!("ls '{}' >/dev/null 2>&1", protected.display()),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn seatbelt_allows_workspace_and_denies_external_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let external_file = external.path().join("denied.txt");
        let runtime = NativeRuntime;
        let spec = ProcessSpec {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "echo allowed > allowed.txt; echo denied > '{}'",
                    external_file.display()
                ),
            ],
            cwd_uri: workspace_uri.clone(),
            environment_handles: Default::default(),
            timeout: Duration::from_secs(5),
            network_enabled: false,
            browser_compatible: false,
            readable_root_uris: vec![workspace_uri.clone()],
            writable_root_uris: vec![workspace_uri],
            denied_read_uris: vec![],
            output_limit_bytes: 4096,
            #[cfg(unix)]
            pinned_cwd: None,
        };
        let output = runtime.execute(spec).await.unwrap();
        assert!(
            workspace.path().join("allowed.txt").exists(),
            "sandbox output: {output:?}"
        );
        assert!(!external_file.exists());
        assert_ne!(output.exit_code, Some(0));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn browser_compatible_seatbelt_still_denies_external_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let external_file = external.path().join("denied.txt");
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "sh".into(),
                args: vec![
                    "-c".into(),
                    format!(
                        "echo allowed > allowed.txt; echo denied > '{}'",
                        external_file.display()
                    ),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: true,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert!(workspace.path().join("allowed.txt").exists());
        assert!(!external_file.exists());
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn seatbelt_denies_network_by_default() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "nc".into(),
                args: vec!["-z".into(), "127.0.0.1".into(), port.to_string()],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn browser_compatible_seatbelt_denies_network_by_default() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "nc".into(),
                args: vec!["-z".into(), "127.0.0.1".into(), port.to_string()],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: UNIX_SANDBOX_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: true,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_allows_workspace_and_denies_external_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let external_file = external.path().join("denied.txt");
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!(
                        "echo allowed > allowed.txt; echo denied > '{}'",
                        external_file.display()
                    ),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert!(
            workspace.path().join("allowed.txt").exists(),
            "bubblewrap output: {output:?}"
        );
        assert!(!external_file.exists());
        assert_ne!(output.exit_code, Some(0));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_denies_network_by_default() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-c".into(),
                    format!("import socket; socket.create_connection(('127.0.0.1', {port}), 1)"),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                denied_read_uris: vec![],
                output_limit_bytes: 4096,
                #[cfg(unix)]
                pinned_cwd: None,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "bubblewrap output: {output:?}");
    }
}
