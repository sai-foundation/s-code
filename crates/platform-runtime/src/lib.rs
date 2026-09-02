use async_trait::async_trait;
use opencoding_protocol::Capability;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
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
    #[serde(default = "default_output_limit")]
    pub output_limit_bytes: usize,
}

fn default_output_limit() -> usize {
    1024 * 1024
}

#[cfg(unix)]
fn child_process_ids(process_id: u32) -> Vec<u32> {
    let Ok(output) = std::process::Command::new("/usr/bin/pgrep")
        .args(["-P", &process_id.to_string()])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[cfg(unix)]
fn kill_process_tree(process_id: u32) {
    fn collect(process_id: u32, descendants: &mut Vec<u32>) {
        for child in child_process_ids(process_id) {
            collect(child, descendants);
            descendants.push(child);
        }
    }

    let mut descendants = Vec::new();
    collect(process_id, &mut descendants);
    for target in descendants.into_iter().chain(std::iter::once(process_id)) {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-KILL", &target.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn sandbox_path(program: &str) -> String {
    const SYSTEM_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
    let Some(parent) = std::path::Path::new(program).parent() else {
        return SYSTEM_PATH.into();
    };
    let parent = parent.to_string_lossy();
    if parent.is_empty() || SYSTEM_PATH.split(':').any(|entry| entry == parent) {
        SYSTEM_PATH.into()
    } else {
        format!("{parent}:{SYSTEM_PATH}")
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
            let mut profile = if spec.browser_compatible {
                // Chromium requires ambient macOS IPC services that are not
                // practical to enumerate. Start permissive, then re-apply the
                // product's network and filesystem write boundaries below.
                String::from("(version 1)(allow default)(deny network*)(deny file-write*)")
            } else {
                String::from(
                    "(version 1)(deny default)(import \"system.sb\")(allow process*)(allow signal (target children))(allow sysctl-read)(allow mach-lookup)(allow file-read-metadata)",
                )
            };
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
            if let Some(xcode_read_root) = macos_xcode_read_root() {
                profile.push_str(&format!(
                    "(allow file-read* (subpath {}))",
                    seatbelt_string(&xcode_read_root)
                ));
            }
            for library in macos_dynamic_libraries(std::path::Path::new(&spec.program)) {
                profile.push_str(&format!(
                    "(allow file-read* (literal {}))",
                    seatbelt_string(&library)
                ));
                if let Some(parent) = library.parent() {
                    profile.push_str(&format!(
                        "(allow file-read* (subpath {}))",
                        seatbelt_string(parent)
                    ));
                }
                if let Ok(canonical) = library.canonicalize()
                    && canonical != library
                {
                    profile.push_str(&format!(
                        "(allow file-read* (literal {}))",
                        seatbelt_string(&canonical)
                    ));
                    if let Some(parent) = canonical.parent() {
                        profile.push_str(&format!(
                            "(allow file-read* (subpath {}))",
                            seatbelt_string(parent)
                        ));
                    }
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

    async fn execute(&self, mut spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
        let cwd = Self::uri_path(&spec.cwd_uri)?
            .canonicalize()
            .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))?;
        let sandbox_temp = tempfile::Builder::new()
            .prefix("opencoding-sandbox.")
            .tempdir()
            .map_err(|error| RuntimeError::Execution(error.to_string()))?;
        let sandbox_temp_uri = url::Url::from_directory_path(sandbox_temp.path())
            .map_err(|_| RuntimeError::Execution("sandbox temp path is not absolute".into()))?
            .to_string();
        spec.writable_root_uris.push(sandbox_temp_uri);
        let mut command = Self::command(&spec)?;
        #[cfg(unix)]
        command.process_group(0);
        command
            .current_dir(cwd)
            .env_clear()
            .env("PATH", sandbox_path(&spec.program))
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
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Execution("failed to capture stdout".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Execution("failed to capture stderr".into()))?;
        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let status = match tokio::time::timeout(spec.timeout, child.wait()).await {
            Ok(status) => status.map_err(|error| RuntimeError::Execution(error.to_string()))?,
            Err(_) => {
                #[cfg(unix)]
                if let Some(process_id) = process_id {
                    kill_process_tree(process_id);
                }
                let _ = child.wait().await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(RuntimeError::Execution("process timed out".into()));
            }
        };
        let stdout = stdout_task
            .await
            .map_err(|error| RuntimeError::Execution(error.to_string()))?
            .map_err(|error| RuntimeError::Execution(error.to_string()))?;
        let stderr = stderr_task
            .await
            .map_err(|error| RuntimeError::Execution(error.to_string()))?
            .map_err(|error| RuntimeError::Execution(error.to_string()))?;
        let (stdout, stdout_cut) = limited(&stdout, spec.output_limit_bytes);
        let remaining = spec.output_limit_bytes.saturating_sub(stdout.len());
        let (stderr, stderr_cut) = limited(&stderr, remaining);
        Ok(ProcessOutput {
            exit_code: status.code(),
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

    #[cfg(target_os = "macos")]
    const XCRUN_TEST_TIMEOUT: Duration = Duration::from_secs(15);

    #[test]
    fn sandbox_path_includes_the_resolved_program_directory() {
        assert_eq!(
            sandbox_path("/opt/tools/bin/npm"),
            "/opt/tools/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
        );
        assert_eq!(
            sandbox_path("/usr/bin/python3"),
            "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
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
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert!(!workspace.path().join("workspace-denied").exists());
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
                browser_compatible: false,
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
    async fn timeout_kills_the_entire_process_tree() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let child_pid_path = workspace.path().join("child.pid");
        let result = NativeRuntime
            .execute(ProcessSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "sleep 30 & echo $! > child.pid; wait".into()],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: Duration::from_millis(250),
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
            })
            .await;
        assert!(
            matches!(result, Err(RuntimeError::Execution(message)) if message == "process timed out")
        );
        let child_pid = std::fs::read_to_string(child_pid_path)
            .unwrap()
            .trim()
            .to_owned();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            let output = std::process::Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &child_pid])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || state.trim().is_empty() || state.trim().starts_with('Z')
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child process {child_pid} survived timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
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
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-c".into(),
                    concat!(
                        "import os, signal; ",
                        "os.killpg(os.getpgrp(), signal.SIGKILL)"
                    )
                    .into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: XCRUN_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
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
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-c".into(),
                    "import subprocess; subprocess.run(['/usr/bin/true'], start_new_session=True, check=True)"
                        .into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: XCRUN_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn sandboxed_python_can_reap_timed_out_grandchildren() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let output = NativeRuntime
            .execute(ProcessSpec {
                program: "/usr/bin/python3".into(),
                args: vec![
                    "-c".into(),
                    concat!(
                        "import subprocess, sys; ",
                        "\ntry:\n subprocess.run([sys.executable,'-c','import time; time.sleep(5)'], ",
                        "stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=.1, check=False)\n",
                        "except subprocess.TimeoutExpired:\n print('reaped')\n"
                    )
                    .into(),
                ],
                cwd_uri: workspace_uri.clone(),
                environment_handles: Default::default(),
                timeout: XCRUN_TEST_TIMEOUT,
                network_enabled: false,
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "sandbox output: {output:?}");
        assert_eq!(output.stdout.trim(), "reaped", "sandbox output: {output:?}");
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
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: true,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
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
                output_limit_bytes: 4096,
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
                timeout: Duration::from_secs(5),
                network_enabled: false,
                browser_compatible: true,
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
                browser_compatible: false,
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
                browser_compatible: false,
                readable_root_uris: vec![workspace_uri.clone()],
                writable_root_uris: vec![workspace_uri],
                output_limit_bytes: 4096,
            })
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "bubblewrap output: {output:?}");
    }
}
