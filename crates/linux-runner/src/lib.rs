use opencoding_platform_runtime::{NativeRuntime, PlatformRuntime, ProcessSpec};
use opencoding_protocol::{
    DurableTask, DurableTaskCheckpoint, DurableTaskCompletion, DurableTaskFailure,
    LINUX_RUNNER_TASK_KIND, LinuxRunnerResult, LinuxRunnerTaskPayload, RunnerProcessStep,
    RunnerStepEvidence,
};
use reqwest::{Client, StatusCode};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use thiserror::Error;

const MAX_STEPS: usize = 32;
const MAX_ARGS: usize = 256;
const MAX_ARG_BYTES: usize = 64 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("invalid task: {0}")]
    InvalidTask(String),
    #[error("control plane error: {0}")]
    ControlPlane(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error("task was cancelled or its lease was revoked")]
    LeaseRevoked,
}

#[derive(Clone, Debug)]
pub struct RunnerConfig {
    pub daemon_url: String,
    pub token: String,
    pub worker_id: String,
    pub allow_network: bool,
    pub poll_interval: Duration,
    pub lease_seconds: u64,
}

impl RunnerConfig {
    pub fn validate(&self) -> Result<(), RunnerError> {
        let url = url::Url::parse(&self.daemon_url)
            .map_err(|error| RunnerError::Configuration(error.to_string()))?;
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
            return Err(RunnerError::Configuration(
                "daemon URL must use HTTPS or explicit loopback HTTP".into(),
            ));
        }
        if url.username() != "" || url.password().is_some() || self.token.trim().is_empty() {
            return Err(RunnerError::Configuration(
                "credentials must use the bearer token, never URL userinfo".into(),
            ));
        }
        if self.worker_id.is_empty()
            || self.worker_id.len() > 128
            || !self
                .worker_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(RunnerError::Configuration("invalid worker id".into()));
        }
        if !(15..=300).contains(&self.lease_seconds) {
            return Err(RunnerError::Configuration(
                "lease must be between 15 and 300 seconds".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct LinuxRunner {
    config: RunnerConfig,
    client: Client,
}

impl LinuxRunner {
    pub fn new(config: RunnerConfig) -> Result<Self, RunnerError> {
        config.validate()?;
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| RunnerError::Configuration(error.to_string()))?;
        Ok(Self { config, client })
    }

    pub async fn run_forever(&self) -> Result<(), RunnerError> {
        ensure_linux()?;
        loop {
            if let Some(task) = self.lease().await? {
                if let Err(error) = self.run_task(task.clone()).await
                    && !matches!(error, RunnerError::LeaseRevoked)
                {
                    let _ = self.fail(&task, &error.to_string(), false, 0).await;
                }
            } else {
                tokio::time::sleep(self.config.poll_interval).await;
            }
        }
    }

    async fn lease(&self) -> Result<Option<DurableTask>, RunnerError> {
        let response = self
            .request(reqwest::Method::POST, "/v1/durable-tasks/lease")
            .json(&serde_json::json!({
                "worker": self.config.worker_id,
                "lease_seconds": self.config.lease_seconds,
                "kind": LINUX_RUNNER_TASK_KIND,
            }))
            .send()
            .await
            .map_err(control_error)?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        decode(response).await.map(Some)
    }

    async fn run_task(&self, task: DurableTask) -> Result<(), RunnerError> {
        if task.kind != LINUX_RUNNER_TASK_KIND {
            return Err(RunnerError::InvalidTask("unexpected task kind".into()));
        }
        let lease_token = task
            .lease_token
            .as_deref()
            .ok_or_else(|| RunnerError::InvalidTask("leased task has no token".into()))?;
        let payload: LinuxRunnerTaskPayload = serde_json::from_value(task.payload.clone())
            .map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
        let workspace = validate_payload(&payload, self.config.allow_network)?;
        verify_snapshot(&workspace, &payload.workspace_snapshot.digest_sha256)?;
        self.post(
            &format!("/v1/durable-tasks/{}/start", task.id.0),
            &serde_json::json!({"lease_token": lease_token}),
        )
        .await?;

        let runtime = NativeRuntime;
        let workspace_uri = payload.workspace_snapshot.root_uri.clone();
        let mut evidence = Vec::with_capacity(payload.tool_plan.len());
        let started = Instant::now();
        for (index, step) in payload.tool_plan.iter().enumerate() {
            let step_started = Instant::now();
            let execution = runtime.execute(process_spec(&payload, step, &workspace_uri));
            tokio::pin!(execution);
            let output = loop {
                tokio::select! {
                    result = &mut execution => break result.map_err(|error| RunnerError::Execution(error.to_string()))?,
                    _ = tokio::time::sleep(Duration::from_secs(self.config.lease_seconds / 3)) => {
                        if self.renew(&task, lease_token).await.is_err() {
                            return Err(RunnerError::LeaseRevoked);
                        }
                    }
                }
            };
            evidence.push(RunnerStepEvidence {
                step: index,
                program: step.program.clone(),
                exit_code: output.exit_code,
                stdout: output.stdout,
                stderr: output.stderr,
                truncated: output.truncated,
                duration_millis: millis(step_started.elapsed()),
            });
            let runner_cost = cost(started.elapsed(), task.max_runner_cost_micros);
            self.checkpoint(&task, lease_token, index + 1, runner_cost)
                .await?;
            if output.exit_code != Some(0) {
                return self
                    .fail(
                        &task,
                        "runner process returned a non-zero exit code",
                        false,
                        runner_cost,
                    )
                    .await;
            }
        }
        let result = LinuxRunnerResult {
            schema_version: 1,
            snapshot_digest_sha256: payload.workspace_snapshot.digest_sha256,
            steps: evidence,
        };
        let completion = DurableTaskCompletion {
            lease_token: lease_token.into(),
            result: serde_json::to_value(result).expect("runner result serializes"),
            consumed_cost_micros: task.consumed_cost_micros,
            consumed_runner_cost_micros: cost(started.elapsed(), task.max_runner_cost_micros),
        };
        self.post(
            &format!("/v1/durable-tasks/{}/complete", task.id.0),
            &completion,
        )
        .await
        .map(|_| ())
    }

    async fn renew(&self, task: &DurableTask, token: &str) -> Result<(), RunnerError> {
        self.post(
            &format!("/v1/durable-tasks/{}/renew", task.id.0),
            &serde_json::json!({
                "lease_token": token,
                "lease_seconds": self.config.lease_seconds,
            }),
        )
        .await
        .map(|_| ())
    }

    async fn checkpoint(
        &self,
        task: &DurableTask,
        token: &str,
        completed_steps: usize,
        runner_cost: u64,
    ) -> Result<(), RunnerError> {
        let checkpoint = DurableTaskCheckpoint {
            lease_token: token.into(),
            checkpoint: serde_json::json!({
                "schema_version": 1,
                "completed_steps": completed_steps,
                "snapshot_digest_sha256": task.payload["workspace_snapshot"]["digest_sha256"],
            }),
            consumed_cost_micros: task.consumed_cost_micros,
            consumed_runner_cost_micros: runner_cost,
        };
        self.post(
            &format!("/v1/durable-tasks/{}/checkpoint", task.id.0),
            &checkpoint,
        )
        .await
        .map(|_| ())
    }

    async fn fail(
        &self,
        task: &DurableTask,
        message: &str,
        retryable: bool,
        runner_cost: u64,
    ) -> Result<(), RunnerError> {
        let token = task
            .lease_token
            .as_deref()
            .ok_or(RunnerError::LeaseRevoked)?;
        let failure = DurableTaskFailure {
            lease_token: token.into(),
            error: message.chars().take(512).collect(),
            retryable,
            consumed_cost_micros: task.consumed_cost_micros,
            consumed_runner_cost_micros: runner_cost,
        };
        self.post(&format!("/v1/durable-tasks/{}/fail", task.id.0), &failure)
            .await
            .map(|_| ())
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(
                method,
                format!("{}{}", self.config.daemon_url.trim_end_matches('/'), path),
            )
            .bearer_auth(&self.config.token)
    }

    async fn post<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<DurableTask, RunnerError> {
        let response = self
            .request(reqwest::Method::POST, path)
            .json(body)
            .send()
            .await
            .map_err(control_error)?;
        decode(response).await
    }
}

fn ensure_linux() -> Result<(), RunnerError> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(RunnerError::Configuration(
            "the concrete runner requires Linux with Bubblewrap; other platforms implement the same task protocol through a new provider".into(),
        ))
    }
}

fn validate_payload(
    payload: &LinuxRunnerTaskPayload,
    allow_network: bool,
) -> Result<PathBuf, RunnerError> {
    if payload.schema_version != 1 {
        return Err(RunnerError::InvalidTask(
            "unsupported schema version".into(),
        ));
    }
    if payload.tool_plan.is_empty() || payload.tool_plan.len() > MAX_STEPS {
        return Err(RunnerError::InvalidTask(
            "tool plan must contain 1-32 steps".into(),
        ));
    }
    if payload.network_enabled && !allow_network {
        return Err(RunnerError::InvalidTask(
            "runner policy denies network".into(),
        ));
    }
    if !payload.secret_handles.is_empty() {
        return Err(RunnerError::InvalidTask(
            "secret redemption is unavailable in the Linux MVP runner".into(),
        ));
    }
    if !(1..=MAX_OUTPUT_BYTES).contains(&payload.output_limit_bytes) {
        return Err(RunnerError::InvalidTask("invalid output limit".into()));
    }
    if payload.workspace_snapshot.digest_sha256.len() != 64
        || !payload
            .workspace_snapshot
            .digest_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RunnerError::InvalidTask("invalid snapshot digest".into()));
    }
    for step in &payload.tool_plan {
        if !step.program.starts_with('/')
            || step.program.len() > 4096
            || step.args.len() > MAX_ARGS
            || step.timeout_seconds == 0
            || step.timeout_seconds > 3600
            || step.args.iter().map(String::len).sum::<usize>() > MAX_ARG_BYTES
            || step.args.iter().any(|arg| arg.as_bytes().contains(&0))
        {
            return Err(RunnerError::InvalidTask("invalid process step".into()));
        }
    }
    let url = url::Url::parse(&payload.workspace_snapshot.root_uri)
        .map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
    if url.scheme() != "file" {
        return Err(RunnerError::InvalidTask(
            "snapshot must use a file URI".into(),
        ));
    }
    let path = url
        .to_file_path()
        .map_err(|_| RunnerError::InvalidTask("invalid snapshot file URI".into()))?;
    path.canonicalize()
        .map_err(|error| RunnerError::InvalidTask(error.to_string()))
}

fn process_spec(
    payload: &LinuxRunnerTaskPayload,
    step: &RunnerProcessStep,
    workspace_uri: &str,
) -> ProcessSpec {
    ProcessSpec {
        program: step.program.clone(),
        args: step.args.clone(),
        cwd_uri: workspace_uri.into(),
        environment_handles: Default::default(),
        timeout: Duration::from_secs(step.timeout_seconds),
        network_enabled: payload.network_enabled,
        readable_root_uris: vec![workspace_uri.into()],
        writable_root_uris: vec![workspace_uri.into()],
        output_limit_bytes: payload.output_limit_bytes,
    }
}

pub fn snapshot_digest(root: &Path) -> Result<String, RunnerError> {
    let root = root
        .canonicalize()
        .map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
    let mut files = Vec::new();
    collect_files(&root, &root, &mut files)?;
    files.sort();
    let mut total = 0_u64;
    let mut hasher = Sha256::new();
    for relative in files {
        let path = root.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RunnerError::InvalidTask(
                "snapshot may contain only regular files and directories".into(),
            ));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(RunnerError::InvalidTask(
                "snapshot file is too large".into(),
            ));
        }
        total = total.saturating_add(metadata.len());
        if total > MAX_SNAPSHOT_BYTES {
            return Err(RunnerError::InvalidTask("snapshot is too large".into()));
        }
        let bytes = fs::read(&path).map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
        let relative = relative.to_string_lossy();
        hasher.update((relative.len() as u64).to_be_bytes());
        hasher.update(relative.as_bytes());
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), RunnerError> {
    let entries =
        fs::read_dir(directory).map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
        let kind = entry
            .file_type()
            .map_err(|error| RunnerError::InvalidTask(error.to_string()))?;
        if kind.is_symlink() {
            return Err(RunnerError::InvalidTask(
                "snapshot symlinks are forbidden".into(),
            ));
        }
        if kind.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if kind.is_file() {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|error| RunnerError::InvalidTask(error.to_string()))?
                    .to_path_buf(),
            );
            if files.len() > MAX_FILES {
                return Err(RunnerError::InvalidTask(
                    "snapshot has too many files".into(),
                ));
            }
        } else {
            return Err(RunnerError::InvalidTask(
                "special snapshot file is forbidden".into(),
            ));
        }
    }
    Ok(())
}

fn verify_snapshot(root: &Path, expected: &str) -> Result<(), RunnerError> {
    if snapshot_digest(root)? != expected.to_ascii_lowercase() {
        return Err(RunnerError::InvalidTask("snapshot digest mismatch".into()));
    }
    Ok(())
}

fn cost(elapsed: Duration, maximum: u64) -> u64 {
    u64::try_from(elapsed.as_micros())
        .unwrap_or(u64::MAX)
        .min(maximum)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn control_error(error: reqwest::Error) -> RunnerError {
    RunnerError::ControlPlane(error.to_string())
}

async fn decode(response: reqwest::Response) -> Result<DurableTask, RunnerError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(RunnerError::ControlPlane(format!(
            "HTTP {status}: {}",
            body.chars().take(512).collect::<String>()
        )));
    }
    response.json().await.map_err(control_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opencoding_protocol::{RunnerProcessStep, WorkspaceSnapshot};

    fn payload(root: &Path) -> LinuxRunnerTaskPayload {
        LinuxRunnerTaskPayload {
            schema_version: 1,
            workspace_snapshot: WorkspaceSnapshot {
                root_uri: url::Url::from_directory_path(root).unwrap().to_string(),
                digest_sha256: snapshot_digest(root).unwrap(),
            },
            tool_plan: vec![RunnerProcessStep {
                program: "/bin/true".into(),
                args: vec![],
                timeout_seconds: 5,
            }],
            network_enabled: false,
            secret_handles: vec![],
            output_limit_bytes: 4096,
        }
    }

    #[test]
    fn payload_is_bounded_and_network_fails_closed() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("a.txt"), "hello").unwrap();
        let mut value = payload(workspace.path());
        assert!(validate_payload(&value, false).is_ok());
        value.network_enabled = true;
        assert!(matches!(
            validate_payload(&value, false),
            Err(RunnerError::InvalidTask(_))
        ));
        value.network_enabled = false;
        value.secret_handles.push("prod-token".into());
        assert!(matches!(
            validate_payload(&value, false),
            Err(RunnerError::InvalidTask(_))
        ));
    }

    #[test]
    fn snapshot_digest_detects_mutation_and_rejects_symlinks() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("a.txt"), "before").unwrap();
        let digest = snapshot_digest(workspace.path()).unwrap();
        verify_snapshot(workspace.path(), &digest).unwrap();
        fs::write(workspace.path().join("a.txt"), "after").unwrap();
        assert!(verify_snapshot(workspace.path(), &digest).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("a.txt", workspace.path().join("link")).unwrap();
            assert!(snapshot_digest(workspace.path()).is_err());
        }
    }

    #[test]
    fn transport_requires_https_or_loopback() {
        let config = RunnerConfig {
            daemon_url: "http://control.example".into(),
            token: "token".into(),
            worker_id: "runner-1".into(),
            allow_network: false,
            poll_interval: Duration::from_secs(1),
            lease_seconds: 30,
        };
        assert!(config.validate().is_err());
        let mut secure = config;
        secure.daemon_url = "https://control.example".into();
        assert!(secure.validate().is_ok());
        secure.daemon_url = "http://127.0.0.1:4096".into();
        assert!(secure.validate().is_ok());
    }

    #[test]
    fn machine_requests_do_not_impersonate_browser_origins() {
        let runner = LinuxRunner::new(RunnerConfig {
            daemon_url: "http://127.0.0.1:4096".into(),
            token: "runner-secret".into(),
            worker_id: "runner-1".into(),
            allow_network: false,
            poll_interval: Duration::from_secs(1),
            lease_seconds: 30,
        })
        .unwrap();
        let request = runner
            .request(reqwest::Method::POST, "/v1/durable-tasks/lease")
            .build()
            .unwrap();
        assert!(request.headers().get("origin").is_none());
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer runner-secret"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn worker_executes_a_leased_task_through_real_daemon_and_bubblewrap() {
        use opencoding_daemon::{AppState, app};
        use opencoding_protocol::{
            CreateDurableTask, DurableTaskStatus, Id, LINUX_RUNNER_TASK_KIND, Scope,
        };
        use opencoding_storage::Store;

        let store = Store::in_memory().await.unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("input.txt"), "source").unwrap();
        let task_payload = payload(workspace.path());
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("actor".into()),
            goal_id: None,
            task_id: None,
        };
        let mut task_payload = task_payload;
        task_payload.tool_plan = vec![RunnerProcessStep {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf complete > result.txt".into()],
            timeout_seconds: 10,
        }];
        let task = store
            .create_durable_task(CreateDurableTask {
                scope: scope.clone(),
                kind: LINUX_RUNNER_TASK_KIND.into(),
                payload: serde_json::to_value(task_payload).unwrap(),
                idempotency_key: "runner-e2e".into(),
                max_attempts: 1,
                max_runtime_seconds: 60,
                max_cost_micros: 0,
                max_runner_cost_micros: 60_000_000,
            })
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_store = store.clone();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app(AppState::new("runner-secret", server_store, 0)),
            )
            .await
            .unwrap();
        });
        let runner = LinuxRunner::new(RunnerConfig {
            daemon_url: format!("http://{address}"),
            token: "runner-secret".into(),
            worker_id: "linux-e2e".into(),
            allow_network: false,
            poll_interval: Duration::from_millis(10),
            lease_seconds: 30,
        })
        .unwrap();
        let leased = runner.lease().await.unwrap().unwrap();
        assert_eq!(leased.id, task.id);
        runner.run_task(leased).await.unwrap();
        assert_eq!(
            fs::read_to_string(workspace.path().join("result.txt")).unwrap(),
            "complete"
        );
        let completed = store.get_durable_task(&scope, &task.id).await.unwrap();
        assert_eq!(completed.status, DurableTaskStatus::Succeeded);
        assert!(completed.result.is_some());
        server.abort();
    }
}
