use async_trait::async_trait;
use opencoding_protocol::Capability;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use thiserror::Error;

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
    pub readable_root_uris: Vec<String>,
    #[serde(default)]
    pub writable_root_uris: Vec<String>,
    #[serde(default = "default_output_limit")]
    pub output_limit_bytes: usize,
}

fn default_output_limit() -> usize {
    1024 * 1024
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
                "(version 1)(deny default)(import \"system.sb\")(allow process*)(allow sysctl-read)(allow mach-lookup)(allow file-read-metadata)",
            );
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
                "/usr",
                "/bin",
                "/sbin",
                "/Library",
                "/private/etc",
                "/dev",
            ] {
                profile.push_str(&format!("(allow file-read* (subpath \"{system}\"))"));
            }
            for library in macos_dynamic_libraries(std::path::Path::new(&spec.program)) {
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
            for uri in &spec.writable_root_uris {
                let path = Self::uri_path(uri)?
                    .canonicalize()
                    .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
                profile.push_str(&format!(
                    "(allow file-read* file-write* (subpath {}))",
                    seatbelt_string(&path)
                ));
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
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
            ]);
            if spec.network_enabled {
                command.arg("--share-net");
            }
            for uri in &spec.writable_root_uris {
                let path = Self::uri_path(uri)?;
                command.arg("--bind").arg(&path).arg(&path);
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

#[cfg(target_os = "macos")]
fn macos_dynamic_libraries(program: &std::path::Path) -> Vec<PathBuf> {
    let Ok(output) = std::process::Command::new("/usr/bin/otool")
        .arg("-L")
        .arg(program)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .collect()
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
            maturity: opencoding_protocol::CapabilityMaturity::Preview,
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
        let cwd = Self::uri_path(&spec.cwd_uri)?
            .canonicalize()
            .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
        let mut command = Self::command(&spec)?;
        command
            .current_dir(cwd)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
            .kill_on_drop(true);
        for (name, value) in &spec.environment_handles {
            command.env(name, value);
        }
        let output = tokio::time::timeout(spec.timeout, command.output())
            .await
            .map_err(|_| RuntimeError::Execution("process timed out".into()))?
            .map_err(|e| RuntimeError::Execution(e.to_string()))?;
        let (stdout, stdout_cut) = limited(&output.stdout, spec.output_limit_bytes);
        let remaining = spec.output_limit_bytes.saturating_sub(stdout.len());
        let (stderr, stderr_cut) = limited(&output.stderr, remaining);
        Ok(ProcessOutput {
            exit_code: output.status.code(),
            stdout,
            stderr,
            truncated: stdout_cut || stderr_cut,
        })
    }

    async fn cancel_process_tree(&self, _process_id: &str) -> Result<(), RuntimeError> {
        Err(RuntimeError::CapabilityUnavailable(
            "process registry not initialized".into(),
        ))
    }
}

fn limited(bytes: &[u8], limit: usize) -> (String, bool) {
    let selected = &bytes[..bytes.len().min(limit)];
    (
        String::from_utf8_lossy(selected).into_owned(),
        bytes.len() > limit,
    )
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
                    readable_root_uris: vec![workspace_uri],
                    writable_root_uris: vec![],
                    output_limit_bytes: 4096,
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
                readable_root_uris: vec![workspace_uri],
                writable_root_uris: vec![],
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(!workspace.path().join("denied.txt").exists());
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
            readable_root_uris: vec![workspace_uri.clone()],
            writable_root_uris: vec![workspace_uri],
            output_limit_bytes: 4096,
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
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
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
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
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
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "bubblewrap output: {output:?}");
    }
}
