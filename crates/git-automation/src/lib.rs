use opencoding_protocol::{Id, Scope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_COMMAND_STDOUT_SCAN_LIMIT: usize = 8 * 1024 * 1024;
const GIT_COMMAND_STDOUT_RETAIN_LIMIT: usize = 4 * 1024 * 1024;
const GIT_DIFF_SCAN_LIMIT: usize = 64 * 1024 * 1024;
const GIT_STDERR_SCAN_LIMIT: usize = 64 * 1024;
const GIT_STDERR_RETAIN_LIMIT: usize = 4097;
const CODEOWNERS_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
struct CapturedStream {
    retained: Vec<u8>,
    sha256: String,
    total: usize,
}

#[derive(Debug)]
struct BoundedCommandOutput {
    status: ExitStatus,
    stdout: CapturedStream,
    stderr: CapturedStream,
}

#[derive(Debug, Error)]
pub enum GitError {
    #[error("invalid repository: {0}")]
    InvalidRepository(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("git command failed: {0}")]
    Command(String),
    #[error("working diff changed since approval")]
    DiffChanged,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub porcelain_v2: String,
    pub head_oid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiffSnapshot {
    pub unified_diff: String,
    pub sha256: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitRequest {
    pub message: String,
    pub paths: Vec<String>,
    pub expected_diff_hash: String,
    pub scope: Scope,
    pub session_id: Id,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitResult {
    pub oid: String,
    pub branch: String,
    pub diff_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeResult {
    pub branch: String,
    pub workspace_uri: String,
}

fn validate_trailer_value(label: &str, value: &str) -> Result<(), GitError> {
    if value.is_empty()
        || value.len() > 4_096
        || value.chars().any(|character| character.is_control())
    {
        return Err(GitError::InvalidArgument(format!(
            "commit {label} identifier must be a non-empty, bounded single-line value"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewerSuggestions {
    pub reviewers: Vec<String>,
    pub codeowners_path: Option<String>,
    pub matched_paths: Vec<String>,
}

#[derive(Clone)]
pub struct GitService {
    root: PathBuf,
    git_binary: PathBuf,
    #[cfg(unix)]
    root_directory: Arc<std::fs::File>,
}

impl GitService {
    pub fn open(workspace_uri: &str) -> Result<Self, GitError> {
        let url = url::Url::parse(workspace_uri)
            .map_err(|error| GitError::InvalidRepository(error.to_string()))?;
        if url.scheme() != "file" {
            return Err(GitError::InvalidRepository(
                "only file:// workspaces are supported".into(),
            ));
        }
        let root = url
            .to_file_path()
            .map_err(|_| GitError::InvalidRepository(workspace_uri.into()))?;
        let root = dunce::canonicalize(root)?;
        let git_binary = find_git_binary(&root)?;
        #[cfg(unix)]
        let root_directory = Arc::new(open_directory_without_following(&root)?);
        let service = Self {
            git_binary,
            root,
            #[cfg(unix)]
            root_directory,
        };
        let repository_root = service.text(["rev-parse", "--show-toplevel"])?;
        let repository_root = dunce::canonicalize(repository_root.trim())?;
        if repository_root != service.root {
            return Err(GitError::InvalidRepository(
                "the authorized workspace must be the repository root".into(),
            ));
        }
        Ok(service)
    }

    pub fn status(&self) -> Result<GitStatus, GitError> {
        Ok(GitStatus {
            branch: self.text(["branch", "--show-current"])?.trim().into(),
            porcelain_v2: self.text(["status", "--porcelain=v2", "--branch"])?,
            head_oid: self.text(["rev-parse", "HEAD"])?.trim().into(),
        })
    }

    pub fn diff(&self, paths: &[String], max_bytes: usize) -> Result<DiffSnapshot, GitError> {
        self.diff_revision(None, paths, max_bytes)
    }

    pub fn diff_revision(
        &self,
        revision: Option<&str>,
        paths: &[String],
        max_bytes: usize,
    ) -> Result<DiffSnapshot, GitError> {
        validate_paths(paths)?;
        let mut args = vec![
            "diff".to_owned(),
            "--binary".to_owned(),
            "--no-ext-diff".to_owned(),
            "--no-textconv".to_owned(),
        ];
        if let Some(revision) = revision {
            validate_revision(revision)?;
            args.push(revision.into());
        }
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        let max_bytes = max_bytes.clamp(1, 1024 * 1024);
        let (selected, hash, truncated) = if revision.is_none() {
            self.bounded_working_diff(args, paths, max_bytes)?
        } else {
            self.bounded_git_diff(args, max_bytes, None)?
        };
        Ok(DiffSnapshot {
            unified_diff: String::from_utf8_lossy(&selected).into_owned(),
            sha256: hash,
            truncated,
        })
    }

    pub fn suggest_reviewers(
        &self,
        paths: &[String],
        team_reviewers: &[String],
    ) -> Result<ReviewerSuggestions, GitError> {
        validate_paths(paths)?;
        if paths.len() > 1024 || team_reviewers.len() > 64 {
            return Err(GitError::InvalidArgument(
                "reviewer suggestion input is too large".into(),
            ));
        }
        let mut reviewers = std::collections::BTreeSet::new();
        for reviewer in team_reviewers {
            validate_reviewer(reviewer)?;
            reviewers.insert(reviewer.clone());
        }
        let codeowners = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"]
            .into_iter()
            .find_map(|relative| {
                self.read_optional_codeowners(relative)
                    .transpose()
                    .map(|result| result.map(|contents| (relative, contents)))
            })
            .transpose()?;
        let mut matched_paths = Vec::new();
        let mut codeowners_path = None;
        if let Some((relative, contents)) = codeowners {
            let rules = parse_codeowners(&contents)?;
            codeowners_path = Some(relative.into());
            for changed_path in paths {
                let mut owners = None;
                for (pattern, candidates) in &rules {
                    if codeowners_matches(pattern, changed_path) {
                        owners = Some(candidates);
                    }
                }
                if let Some(owners) = owners {
                    matched_paths.push(changed_path.clone());
                    reviewers.extend(owners.iter().cloned());
                }
            }
        }
        Ok(ReviewerSuggestions {
            reviewers: reviewers.into_iter().collect(),
            codeowners_path,
            matched_paths,
        })
    }

    #[cfg(unix)]
    fn read_optional_codeowners(&self, relative: &str) -> Result<Option<String>, GitError> {
        use rustix::fs::{Mode, OFlags, openat};

        let mut directory = self.root_directory.try_clone()?;
        let components = Path::new(relative).components().collect::<Vec<_>>();
        let (leaf, parents) = components
            .split_last()
            .ok_or_else(|| GitError::InvalidArgument("invalid CODEOWNERS path".into()))?;
        for component in parents {
            let Component::Normal(component) = component else {
                return Err(GitError::InvalidArgument("invalid CODEOWNERS path".into()));
            };
            let descriptor = match openat(
                &directory,
                *component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(descriptor) => descriptor,
                Err(error)
                    if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(GitError::Io(error.into())),
            };
            directory = descriptor.into();
        }
        let Component::Normal(leaf) = leaf else {
            return Err(GitError::InvalidArgument("invalid CODEOWNERS path".into()));
        };
        let descriptor = match openat(
            &directory,
            *leaf,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(descriptor) => descriptor,
            Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(GitError::Io(error.into())),
        };
        read_codeowners_handle(descriptor.into()).map(Some)
    }

    #[cfg(not(unix))]
    fn read_optional_codeowners(&self, relative: &str) -> Result<Option<String>, GitError> {
        let path = self.root.join(relative);
        let mut current = self.root.clone();
        for component in Path::new(relative).components() {
            let Component::Normal(component) = component else {
                return Err(GitError::InvalidArgument("invalid CODEOWNERS path".into()));
            };
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(GitError::InvalidRepository(
                        "CODEOWNERS must not traverse symbolic links".into(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(GitError::Io(error)),
            }
        }
        let canonical = dunce::canonicalize(&path)?;
        if !canonical.starts_with(&self.root) {
            return Err(GitError::InvalidRepository(
                "CODEOWNERS resolves outside the authorized workspace".into(),
            ));
        }
        read_codeowners_handle(std::fs::File::open(canonical)?).map(Some)
    }

    pub fn create_worktree(&self, base: &str, branch: &str) -> Result<WorktreeResult, GitError> {
        validate_ref(base)?;
        validate_managed_branch(branch)?;
        let git_common = self.text(["rev-parse", "--git-common-dir"])?;
        let common = self.resolve_git_path(git_common.trim())?;
        let worktree_root = common.join("opencoding-worktrees");
        std::fs::create_dir_all(&worktree_root)?;
        let directory = worktree_root.join(branch.replace('/', "__"));
        if directory.exists() {
            return Err(GitError::InvalidArgument(
                "managed worktree already exists".into(),
            ));
        }
        self.git([
            "worktree",
            "add",
            "--no-track",
            "-b",
            branch,
            directory
                .to_str()
                .ok_or_else(|| GitError::InvalidArgument("non-UTF-8 worktree path".into()))?,
            base,
        ])?;
        Ok(WorktreeResult {
            branch: branch.into(),
            workspace_uri: url::Url::from_directory_path(directory)
                .map_err(|_| GitError::InvalidArgument("worktree URI".into()))?
                .to_string(),
        })
    }

    pub fn commit(&self, request: CommitRequest) -> Result<CommitResult, GitError> {
        if request.message.trim().is_empty() || request.message.contains('\0') {
            return Err(GitError::InvalidArgument(
                "commit message is empty or invalid".into(),
            ));
        }
        if request.paths.is_empty() {
            return Err(GitError::InvalidArgument(
                "commit paths are required".into(),
            ));
        }
        for (label, value) in [
            ("team", &request.scope.team_id.0),
            ("actor", &request.scope.actor_id.0),
            ("session", &request.session_id.0),
        ] {
            validate_trailer_value(label, value)?;
        }
        if let Some(task_id) = &request.scope.task_id {
            validate_trailer_value("task", &task_id.0)?;
        }
        if let Some(goal_id) = &request.scope.goal_id {
            validate_trailer_value("goal", &goal_id.0)?;
        }
        validate_paths(&request.paths)?;
        let branch = self.text(["branch", "--show-current"])?;
        validate_managed_branch(branch.trim())?;
        let current = self.working_diff_hash(&request.paths)?;
        if current != request.expected_diff_hash {
            return Err(GitError::DiffChanged);
        }
        let (_temporary, index) = self.temporary_index()?;
        let mut add = vec!["add".to_owned(), "--all".to_owned(), "--".to_owned()];
        add.extend(request.paths.iter().cloned());
        self.git_with_index(add, &index)?;
        let staged = self.staged_hash_with_index(&request.paths, Some(&index))?;
        if staged != request.expected_diff_hash {
            return Err(GitError::DiffChanged);
        }
        let trailers = format!(
            "\n\nOpencoding-Team: {}\nOpencoding-Actor: {}\nOpencoding-Session: {}{}{}",
            request.scope.team_id.0,
            request.scope.actor_id.0,
            request.session_id.0,
            request
                .scope
                .task_id
                .as_ref()
                .map(|id| "\nOpencoding-Task: ".to_owned() + &id.0)
                .unwrap_or_default(),
            request
                .scope
                .goal_id
                .as_ref()
                .map(|id| "\nOpencoding-Goal: ".to_owned() + &id.0)
                .unwrap_or_default(),
        );
        self.git_with_index(
            [
                "-c",
                "user.name=Opencoding Agent",
                "-c",
                "user.email=agent@opencoding.local",
                "commit",
                "--no-verify",
                "-m",
                &(request.message + &trailers),
            ],
            &index,
        )?;
        let mut refresh = vec!["reset".to_owned(), "HEAD".to_owned(), "--".to_owned()];
        refresh.extend(request.paths.iter().cloned());
        self.git(refresh)?;
        let status = self.status()?;
        Ok(CommitResult {
            oid: status.head_oid,
            branch: status.branch,
            diff_hash: staged,
        })
    }

    #[cfg(test)]
    fn staged_hash(&self, paths: &[String]) -> Result<String, GitError> {
        self.staged_hash_with_index(paths, None)
    }

    fn staged_hash_with_index(
        &self,
        paths: &[String],
        index: Option<&Path>,
    ) -> Result<String, GitError> {
        validate_paths(paths)?;
        let mut args = vec![
            "diff".to_owned(),
            "--cached".to_owned(),
            "--binary".to_owned(),
            "--no-ext-diff".to_owned(),
            "--no-textconv".to_owned(),
            "--".to_owned(),
        ];
        args.extend(paths.iter().cloned());
        self.hash_git_diff(args, index)
    }

    fn working_diff_hash(&self, paths: &[String]) -> Result<String, GitError> {
        validate_paths(paths)?;
        let args = vec![
            "diff".to_owned(),
            "--binary".to_owned(),
            "--no-ext-diff".to_owned(),
            "--no-textconv".to_owned(),
        ];
        let (_, hash, _) = self.bounded_working_diff(args, paths, 0)?;
        Ok(hash)
    }

    fn hash_git_diff(&self, args: Vec<String>, index: Option<&Path>) -> Result<String, GitError> {
        let (_, hash, _) = self.bounded_git_diff(args, 0, index)?;
        Ok(hash)
    }

    fn temporary_index(&self) -> Result<(tempfile::TempDir, PathBuf), GitError> {
        let directory = tempfile::tempdir()?;
        let index = directory.path().join("index");
        self.git_with_index(["read-tree", "HEAD"], &index)?;
        Ok((directory, index))
    }

    fn bounded_working_diff(
        &self,
        mut args: Vec<String>,
        paths: &[String],
        max_bytes: usize,
    ) -> Result<(Vec<u8>, String, bool), GitError> {
        let (_temporary, index) = self.temporary_index()?;
        let mut intent = vec![
            "add".to_owned(),
            "--intent-to-add".to_owned(),
            "--ignore-removal".to_owned(),
            "--".to_owned(),
        ];
        if paths.is_empty() {
            intent.push(".".into());
        } else {
            intent.extend(paths.iter().cloned());
        }
        self.git_with_index(intent, &index)?;
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        self.bounded_git_diff(args, max_bytes, Some(&index))
    }

    fn text<I, S>(&self, args: I) -> Result<String, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(String::from_utf8_lossy(&self.git(args)?.stdout).into_owned())
    }

    fn git<I, S>(&self, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git_command(args, None)
    }

    fn git_with_index<I, S>(&self, args: I, index: &Path) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git_command(args, Some(index))
    }

    fn git_command<I, S>(&self, args: I, index: Option<&Path>) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.ensure_safe_repository_config()?;
        let mut command = self.hardened_command()?;
        command.args(args);
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let output = self.run_bounded_command(
            command,
            GIT_COMMAND_STDOUT_RETAIN_LIMIT,
            GIT_COMMAND_STDOUT_SCAN_LIMIT,
            GIT_COMMAND_TIMEOUT,
        )?;
        if !output.status.success() {
            return Err(GitError::Command(redact_git_error(&output.stderr.retained)));
        }
        Ok(Output {
            status: output.status,
            stdout: output.stdout.retained,
            stderr: output.stderr.retained,
        })
    }

    fn bounded_git_diff(
        &self,
        args: Vec<String>,
        max_bytes: usize,
        index: Option<&Path>,
    ) -> Result<(Vec<u8>, String, bool), GitError> {
        self.ensure_safe_repository_config()?;
        let mut command = self.hardened_command()?;
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let output =
            self.run_bounded_command(command, max_bytes, GIT_DIFF_SCAN_LIMIT, GIT_COMMAND_TIMEOUT)?;
        if !output.status.success() {
            return Err(GitError::Command(redact_git_error(&output.stderr.retained)));
        }
        Ok((
            output.stdout.retained,
            output.stdout.sha256,
            output.stdout.total > max_bytes,
        ))
    }

    fn run_bounded_command(
        &self,
        mut command: Command,
        stdout_retain_limit: usize,
        stdout_scan_limit: usize,
        timeout: Duration,
    ) -> Result<BoundedCommandOutput, GitError> {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        configure_process_group(&mut command);
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| GitError::Command("git stdout was unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| GitError::Command("git stderr was unavailable".into()))?;
        let stdout_exceeded = Arc::new(AtomicBool::new(false));
        let stderr_exceeded = Arc::new(AtomicBool::new(false));
        let stdout_reader = spawn_bounded_reader(
            stdout,
            stdout_retain_limit,
            stdout_scan_limit,
            stdout_exceeded.clone(),
        );
        let stderr_reader = spawn_bounded_reader(
            stderr,
            GIT_STDERR_RETAIN_LIMIT,
            GIT_STDERR_SCAN_LIMIT,
            stderr_exceeded.clone(),
        );
        let deadline = Instant::now() + timeout;
        let status = loop {
            if stdout_exceeded.load(Ordering::Acquire) || stderr_exceeded.load(Ordering::Acquire) {
                terminate_child(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(GitError::Command(format!(
                    "git output exceeded its safety limit (stdout {stdout_scan_limit} bytes, stderr {GIT_STDERR_SCAN_LIMIT} bytes)"
                )));
            }
            if let Some(status) = child.try_wait()? {
                terminate_process_group(child.id());
                break status;
            }
            if Instant::now() >= deadline {
                terminate_child(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(GitError::Command(format!(
                    "git command exceeded its {} second timeout",
                    timeout.as_secs_f64()
                )));
            }
            thread::sleep(Duration::from_millis(10));
        };
        let stdout = join_reader(stdout_reader)?;
        let stderr = join_reader(stderr_reader)?;
        if stdout_exceeded.load(Ordering::Acquire) || stderr_exceeded.load(Ordering::Acquire) {
            return Err(GitError::Command(format!(
                "git output exceeded its safety limit (stdout {stdout_scan_limit} bytes, stderr {GIT_STDERR_SCAN_LIMIT} bytes)"
            )));
        }
        Ok(BoundedCommandOutput {
            status,
            stdout,
            stderr,
        })
    }

    fn hardened_command(&self) -> Result<Command, GitError> {
        let null_device = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let mut command = Command::new(&self.git_binary);
        command.env_clear();
        if let Some(parent) = self.git_binary.parent()
            && let Ok(path) = std::env::join_paths([parent])
        {
            command.env("PATH", path);
        }
        #[cfg(windows)]
        for key in ["SystemRoot", "WINDIR", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .arg("--no-pager")
            .args([
                "-c",
                if cfg!(windows) {
                    "core.hooksPath=NUL"
                } else {
                    "core.hooksPath=/dev/null"
                },
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
                "-c",
                "submodule.recurse=false",
                "-c",
                "fetch.recurseSubmodules=false",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "gc.auto=0",
            ])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null_device)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("LANG", "C")
            .env("LC_ALL", "C");
        #[cfg(unix)]
        {
            use std::os::unix::{fs::MetadataExt, process::CommandExt};

            let root = self.root.clone();
            let root_directory = self.root_directory.clone();
            command.current_dir("/");
            // Validate the pathname in the child immediately before exec, then
            // enter the already-open repository directory. A renamed/replaced
            // workspace therefore fails closed instead of redirecting Git.
            unsafe {
                command.pre_exec(move || {
                    let path_metadata = std::fs::metadata(&root)?;
                    let opened_metadata = root_directory.metadata()?;
                    if path_metadata.dev() != opened_metadata.dev()
                        || path_metadata.ino() != opened_metadata.ino()
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "repository path no longer names the authorized directory",
                        ));
                    }
                    rustix::process::fchdir(&*root_directory).map_err(std::io::Error::from)
                });
            }
        }
        #[cfg(not(unix))]
        {
            if dunce::canonicalize(&self.root)? != self.root {
                return Err(GitError::InvalidRepository(
                    "repository path no longer names the authorized directory".into(),
                ));
            }
            command.current_dir(&self.root);
        }
        Ok(command)
    }

    fn ensure_safe_repository_config(&self) -> Result<(), GitError> {
        let local_names = self.repository_config_names("--local", "local")?;
        self.reject_dangerous_git_config(&local_names, "local")?;
        if local_names
            .iter()
            .any(|name| name == "extensions.worktreeconfig")
        {
            let mut command = self.hardened_command()?;
            command.args([
                "config",
                "--local",
                "--no-includes",
                "--bool",
                "--get",
                "extensions.worktreeConfig",
            ]);
            let output = self
                .run_bounded_command(command, 16, 16, GIT_COMMAND_TIMEOUT)
                .map_err(|error| GitError::InvalidRepository(error.to_string()))?;
            if !output.status.success() {
                return Err(GitError::InvalidRepository(
                    "extensions.worktreeConfig is not a valid boolean".into(),
                ));
            }
            if String::from_utf8_lossy(&output.stdout.retained).trim() == "true" {
                let worktree_names = self.repository_config_names("--worktree", "worktree")?;
                self.reject_dangerous_git_config(&worktree_names, "worktree")?;
            }
        }
        Ok(())
    }

    fn repository_config_names(&self, scope: &str, label: &str) -> Result<Vec<String>, GitError> {
        let mut command = self.hardened_command()?;
        command.args([
            "config",
            scope,
            "--no-includes",
            "--null",
            "--name-only",
            "--list",
        ]);
        let output = self
            .run_bounded_command(command, 64 * 1024 + 1, 64 * 1024 + 1, GIT_COMMAND_TIMEOUT)
            .map_err(|error| GitError::InvalidRepository(error.to_string()))?;
        if !output.status.success() {
            return Err(GitError::InvalidRepository(redact_git_error(
                &output.stderr.retained,
            )));
        }
        if output.stdout.retained.len() > 64 * 1024 {
            return Err(GitError::InvalidRepository(format!(
                "{label} Git configuration exceeds the 64 KiB safety limit"
            )));
        }
        Ok(output
            .stdout
            .retained
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8_lossy(name).to_ascii_lowercase())
            .collect())
    }

    fn reject_dangerous_git_config(&self, names: &[String], label: &str) -> Result<(), GitError> {
        for name in names {
            if dangerous_git_config_key(name) {
                return Err(GitError::InvalidRepository(format!(
                    "{label} Git configuration enables an external execution surface: {name}"
                )));
            }
        }
        Ok(())
    }

    fn resolve_git_path(&self, path: &str) -> Result<PathBuf, GitError> {
        let path = PathBuf::from(path);
        dunce::canonicalize(if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        })
        .map_err(GitError::Io)
    }
}

#[cfg(unix)]
fn open_directory_without_following(path: &Path) -> Result<std::fs::File, GitError> {
    use rustix::fs::{Mode, OFlags, open};

    open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(Into::into)
    .map_err(|error| GitError::Io(error.into()))
}

fn read_codeowners_handle(mut file: std::fs::File) -> Result<String, GitError> {
    if !file.metadata()?.is_file() {
        return Err(GitError::InvalidRepository(
            "CODEOWNERS must be a regular file".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(16 * 1024);
    file.by_ref()
        .take((CODEOWNERS_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > CODEOWNERS_LIMIT {
        return Err(GitError::InvalidArgument(
            "CODEOWNERS exceeds the 1 MiB limit".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| GitError::InvalidArgument("CODEOWNERS must be valid UTF-8".into()))
}

fn spawn_bounded_reader<R>(
    mut reader: R,
    retain_limit: usize,
    scan_limit: usize,
    exceeded: Arc<AtomicBool>,
) -> thread::JoinHandle<std::io::Result<CapturedStream>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut retained = Vec::with_capacity(retain_limit.min(64 * 1024));
        let mut digest = Sha256::new();
        let mut total = 0usize;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
            total = total.saturating_add(count);
            let remaining = retain_limit.saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..count.min(remaining)]);
            if total > scan_limit {
                exceeded.store(true, Ordering::Release);
                break;
            }
        }
        Ok(CapturedStream {
            retained,
            sha256: digest
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            total,
        })
    })
}

fn join_reader(
    reader: thread::JoinHandle<std::io::Result<CapturedStream>>,
) -> Result<CapturedStream, GitError> {
    reader
        .join()
        .map_err(|_| GitError::Command("git output reader panicked".into()))?
        .map_err(GitError::Io)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group(process_id: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{process_id}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn terminate_process_group(_process_id: u32) {}

fn terminate_child(child: &mut Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn find_git_binary(workspace: &Path) -> Result<PathBuf, GitError> {
    find_git_binary_with_path(workspace, std::env::var_os("PATH").as_deref())
}

fn find_git_binary_with_path(
    workspace: &Path,
    search_path: Option<&OsStr>,
) -> Result<PathBuf, GitError> {
    let mut candidates = Vec::new();
    #[cfg(unix)]
    candidates.extend(
        [
            "/usr/bin/git",
            "/bin/git",
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/home/linuxbrew/.linuxbrew/bin/git",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    #[cfg(windows)]
    for root in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("LocalAppData"),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(PathBuf::from(&root).join("Git/cmd/git.exe"));
        candidates.push(PathBuf::from(root).join("Programs/Git/cmd/git.exe"));
    }
    if let Some(path) = search_path {
        candidates.extend(
            std::env::split_paths(path)
                .filter(|path| path.is_absolute())
                .map(|directory| directory.join(if cfg!(windows) { "git.exe" } else { "git" })),
        );
    }
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let canonical = dunce::canonicalize(candidate)?;
        if canonical.starts_with(workspace) {
            continue;
        }
        #[cfg(unix)]
        if !trusted_unix_executable_path(&canonical) {
            continue;
        }
        return Ok(canonical);
    }
    Err(GitError::InvalidRepository(
        "a trusted git executable was not found outside the authorized workspace".into(),
    ))
}

#[cfg(unix)]
fn trusted_unix_executable_path(candidate: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(binary) = std::fs::metadata(candidate) else {
        return false;
    };
    if !binary.is_file() || binary.permissions().mode() & 0o022 != 0 {
        return false;
    }
    candidate.ancestors().skip(1).all(|ancestor| {
        let Ok(metadata) = std::fs::metadata(ancestor) else {
            return false;
        };
        metadata.permissions().mode() & 0o022 == 0
    })
}

fn dangerous_git_config_key(name: &str) -> bool {
    name.starts_with("filter.")
        || name == "core.fsmonitor"
        || name == "core.hookspath"
        || name == "core.attributesfile"
        || name == "diff.external"
        || name == "include.path"
        || name.starts_with("includeif.")
        || (name.starts_with("diff.")
            && (name.ends_with(".textconv") || name.ends_with(".command")))
        || (name.starts_with("submodule.") && name.ends_with(".update"))
}

fn validate_paths(paths: &[String]) -> Result<(), GitError> {
    for path in paths {
        let value = Path::new(path);
        if value.as_os_str().is_empty()
            || value.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(GitError::InvalidArgument(format!(
                "path escapes repository: {path}"
            )));
        }
    }
    Ok(())
}

fn validate_ref(value: &str) -> Result<(), GitError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.contains(char::is_whitespace)
        || value.contains("..")
    {
        return Err(GitError::InvalidArgument("invalid git ref".into()));
    }
    Ok(())
}

fn validate_revision(value: &str) -> Result<(), GitError> {
    if value.is_empty()
        || value.len() > 256
        || value.starts_with('-')
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-^~".contains(character))
    {
        return Err(GitError::InvalidArgument(
            "invalid git revision or range".into(),
        ));
    }
    Ok(())
}

fn validate_managed_branch(value: &str) -> Result<(), GitError> {
    validate_ref(value)?;
    if !value.starts_with("opencoding/") {
        return Err(GitError::InvalidArgument(
            "branch must use the opencoding/ namespace".into(),
        ));
    }
    Ok(())
}

fn validate_reviewer(value: &str) -> Result<(), GitError> {
    if value.len() < 2
        || value.len() > 128
        || !value.starts_with('@')
        || !value[1..]
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_/.".contains(character))
    {
        return Err(GitError::InvalidArgument(
            "reviewers must be bounded @user or @organization/team identifiers".into(),
        ));
    }
    Ok(())
}

fn parse_codeowners(contents: &str) -> Result<Vec<(String, Vec<String>)>, GitError> {
    let mut rules = Vec::new();
    for raw in contents.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let pattern = fields.next().unwrap_or_default();
        if pattern.starts_with('!') || pattern.contains("..") || pattern.len() > 512 {
            return Err(GitError::InvalidArgument(
                "CODEOWNERS contains an unsupported or unsafe pattern".into(),
            ));
        }
        let owners = fields
            .map(|value| {
                validate_reviewer(value)?;
                Ok(value.to_owned())
            })
            .collect::<Result<Vec<_>, GitError>>()?;
        if !owners.is_empty() {
            rules.push((pattern.to_owned(), owners));
        }
        if rules.len() > 4096 {
            return Err(GitError::InvalidArgument(
                "CODEOWNERS contains too many rules".into(),
            ));
        }
    }
    Ok(rules)
}

fn codeowners_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    if let Some(directory) = pattern.strip_suffix('/') {
        return path == directory || path.starts_with(&format!("{directory}/"));
    }
    if !pattern.contains('/') {
        return path
            .split('/')
            .any(|component| wildcard_matches(pattern, component));
    }
    wildcard_matches(pattern, path)
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for character in pattern {
        let mut current = vec![false; value.len() + 1];
        if *character == b'*' {
            current[0] = previous[0];
        }
        for index in 1..=value.len() {
            current[index] = match character {
                b'*' => previous[index] || current[index - 1],
                b'?' => previous[index - 1] && value[index - 1] != b'/',
                exact => previous[index - 1] && *exact == value[index - 1],
            };
        }
        previous = current;
    }
    previous[value.len()]
}

fn redact_git_error(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() > 4096 {
        let mut end = 4096;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, GitService) {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("a.txt"), "old\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let uri = url::Url::from_directory_path(dir.path())
            .unwrap()
            .to_string();
        let service = GitService::open(&uri).unwrap();
        (dir, service)
    }

    fn scope() -> Scope {
        Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("user".into()),
            goal_id: Some(Id("goal".into())),
            task_id: Some(Id("task".into())),
        }
    }

    #[cfg(unix)]
    fn canary_script(dir: &Path, name: &str, canary: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join(name);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf invoked > '{}'\ncat \"${{1:-/dev/stdin}}\" 2>/dev/null || true\n",
                canary.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script
    }

    #[cfg(unix)]
    fn set_local_config(root: &Path, key: &str, value: &Path) {
        assert!(
            Command::new("git")
                .args(["config", "--local", key])
                .arg(value)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }

    #[test]
    fn git_error_truncation_preserves_utf8_boundaries() {
        let message = format!("{}🙂tail", "a".repeat(4095));
        let redacted = redact_git_error(message.as_bytes());
        assert_eq!(redacted, format!("{}…", "a".repeat(4095)));
        assert!(redacted.is_char_boundary(redacted.len()));
    }

    #[test]
    fn git_diff_streams_and_clamps_untrusted_output_limits() {
        let (dir, service) = repository();
        std::fs::write(dir.path().join("a.txt"), "x".repeat(2 * 1024 * 1024)).unwrap();
        let snapshot = service.diff(&["a.txt".into()], usize::MAX).unwrap();
        assert!(snapshot.truncated);
        assert!(snapshot.unified_diff.len() <= 1024 * 1024);
        assert_eq!(snapshot.sha256.len(), 64);
    }

    #[test]
    fn nested_workspace_cannot_expose_parent_repository_changes() {
        let (directory, _service) = repository();
        let nested = directory.path().join("authorized-subdirectory");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            directory.path().join("outside-sibling.txt"),
            "private change\n",
        )
        .unwrap();
        let uri = url::Url::from_directory_path(&nested).unwrap().to_string();

        assert!(matches!(
            GitService::open(&uri),
            Err(GitError::InvalidRepository(message))
                if message.contains("authorized workspace must be the repository root")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn repository_local_git_on_search_path_is_never_executed() {
        use std::os::unix::fs::PermissionsExt;

        let (directory, _service) = repository();
        let outside = tempfile::tempdir().unwrap();
        let marker = outside.path().join("workspace-git-ran");
        let fake = directory.path().join("git");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf invoked > '{}'\nexit 0\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let root = dunce::canonicalize(directory.path()).unwrap();
        let search = std::env::join_paths([directory.path()]).unwrap();
        let trusted = find_git_binary_with_path(&root, Some(&search)).unwrap();
        assert!(!trusted.starts_with(&root));
        let service = GitService {
            root,
            git_binary: trusted,
            root_directory: Arc::new(open_directory_without_following(directory.path()).unwrap()),
        };

        service.status().unwrap();
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn hostile_git_execution_configuration_is_rejected_before_programs_run() {
        let (dir, service) = repository();
        let canary = dir.path().join("fsmonitor-ran");
        let script = canary_script(dir.path(), "fsmonitor.sh", &canary);
        set_local_config(dir.path(), "core.fsmonitor", &script);
        assert!(matches!(
            service.status(),
            Err(GitError::InvalidRepository(message)) if message.contains("core.fsmonitor")
        ));
        assert!(!canary.exists());

        let (dir, service) = repository();
        let canary = dir.path().join("textconv-ran");
        let script = canary_script(dir.path(), "textconv.sh", &canary);
        std::fs::write(dir.path().join(".gitattributes"), "*.txt diff=hostile\n").unwrap();
        set_local_config(dir.path(), "diff.hostile.textconv", &script);
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        assert!(matches!(
            service.diff(&["a.txt".into()], 10_000),
            Err(GitError::InvalidRepository(message)) if message.contains("diff.hostile.textconv")
        ));
        assert!(!canary.exists());

        let (dir, service) = repository();
        let canary = dir.path().join("clean-filter-ran");
        let script = canary_script(dir.path(), "clean-filter.sh", &canary);
        std::fs::write(dir.path().join(".gitattributes"), "*.txt filter=hostile\n").unwrap();
        set_local_config(dir.path(), "filter.hostile.clean", &script);
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        assert!(matches!(
            service.git(["add", "--", "a.txt"]),
            Err(GitError::InvalidRepository(message)) if message.contains("filter.hostile.clean")
        ));
        assert!(!canary.exists());
    }

    #[cfg(unix)]
    #[test]
    fn git_commands_do_not_follow_a_replaced_repository_path() {
        use std::os::unix::fs::symlink;

        let (directory, service) = repository();
        let original = directory.path().to_path_buf();
        let parked = original.with_extension("authorized-repository");
        let outside = tempfile::tempdir().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(outside.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(outside.path().join("outside.txt"), "outside\n").unwrap();
        std::fs::rename(&original, &parked).unwrap();
        symlink(outside.path(), &original).unwrap();

        assert!(service.status().is_err());
        assert!(service.diff(&[], 1024).is_err());
        assert!(
            service
                .commit(CommitRequest {
                    message: "must not run".into(),
                    paths: vec!["outside.txt".into()],
                    expected_diff_hash: "not-a-real-hash".into(),
                    scope: scope(),
                    session_id: Id("session".into()),
                })
                .is_err()
        );
        assert!(!outside.path().join(".git/index.lock").exists());

        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&parked, &original).unwrap();
    }

    #[test]
    fn commit_hashing_streams_large_binary_diffs_without_retaining_them() {
        let (dir, service) = repository();
        service
            .git(["checkout", "-b", "opencoding/large-binary"])
            .unwrap();
        let bytes = (0..8 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(dir.path().join("a.txt"), bytes).unwrap();

        let displayed = service.diff(&["a.txt".into()], 1024).unwrap();
        let streamed = service.working_diff_hash(&["a.txt".into()]).unwrap();
        assert_eq!(streamed, displayed.sha256);
        assert!(displayed.unified_diff.len() <= 1024);

        service.git(["add", "--", "a.txt"]).unwrap();
        assert_eq!(service.staged_hash(&["a.txt".into()]).unwrap(), streamed);
    }

    #[test]
    fn git_output_scan_budget_fails_closed_without_spooling_to_disk() {
        let (dir, service) = repository();
        std::fs::write(dir.path().join("large.bin"), vec![b'x'; 256 * 1024]).unwrap();
        service.git(["add", "--", "large.bin"]).unwrap();
        service
            .git([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--no-verify",
                "-m",
                "large fixture",
            ])
            .unwrap();

        let mut command = service.hardened_command().unwrap();
        command.args(["cat-file", "blob", "HEAD:large.bin"]);
        let error = service
            .run_bounded_command(command, 1024, 32 * 1024, Duration::from_secs(2))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("output exceeded its safety limit")
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_timeout_kills_the_entire_process_group() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, service) = repository();
        let canary = dir.path().join("late-side-effect");
        let script = dir.path().join("delayed-side-effect.sh");
        let sleep = if Path::new("/bin/sleep").is_file() {
            "/bin/sleep"
        } else {
            "/usr/bin/sleep"
        };
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n{sleep} 0.4\nprintf invoked > '{}'\n",
                canary.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = service.hardened_command().unwrap();
        command.args([
            "-c",
            &format!("alias.opencoding-hang=!{}", script.display()),
            "opencoding-hang",
        ]);
        let error = service
            .run_bounded_command(command, 1024, 4096, Duration::from_millis(50))
            .unwrap_err();
        assert!(error.to_string().contains("timeout"));
        thread::sleep(Duration::from_millis(500));
        assert!(!canary.exists());
    }

    #[test]
    fn reviewer_suggestions_merge_team_ownership_and_last_matching_codeowners_rule() {
        let (dir, service) = repository();
        std::fs::create_dir_all(dir.path().join(".github")).unwrap();
        std::fs::write(
            dir.path().join(".github/CODEOWNERS"),
            "* @fallback\nsrc/* @platform/team\nsrc/security/* @security/reviewers @alice\n",
        )
        .unwrap();
        let suggestions = service
            .suggest_reviewers(
                &["src/security/auth.rs".into(), "README.md".into()],
                &["@team/oncall".into()],
            )
            .unwrap();
        assert_eq!(
            suggestions.codeowners_path.as_deref(),
            Some(".github/CODEOWNERS")
        );
        assert_eq!(
            suggestions.reviewers,
            vec![
                "@alice".to_owned(),
                "@fallback".to_owned(),
                "@security/reviewers".to_owned(),
                "@team/oncall".to_owned()
            ]
        );
        assert_eq!(suggestions.matched_paths.len(), 2);
        assert!(
            service
                .suggest_reviewers(&["src/lib.rs".into()], &["not-a-reviewer".into()])
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn reviewer_suggestions_never_follow_codeowners_symlinks() {
        use std::os::unix::fs::symlink;

        let (dir, service) = repository();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("CODEOWNERS");
        std::fs::write(&outside_file, "* @outside-reader\n").unwrap();
        std::fs::create_dir_all(dir.path().join(".github")).unwrap();
        symlink(&outside_file, dir.path().join(".github/CODEOWNERS")).unwrap();

        assert!(
            service
                .suggest_reviewers(&["src/lib.rs".into()], &[])
                .is_err()
        );

        std::fs::remove_file(dir.path().join(".github/CODEOWNERS")).unwrap();
        std::fs::remove_dir(dir.path().join(".github")).unwrap();
        symlink(outside.path(), dir.path().join(".github")).unwrap();
        assert!(
            service
                .suggest_reviewers(&["src/lib.rs".into()], &[])
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_git_execution_configuration_is_rejected_before_programs_run() {
        let (dir, service) = repository();
        let canary = dir.path().join("worktree-clean-filter-ran");
        let script = canary_script(dir.path(), "worktree-clean-filter.sh", &canary);
        assert!(
            Command::new("git")
                .args(["config", "--local", "extensions.worktreeConfig", "true"])
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .args(["config", "--worktree", "filter.hostile.clean"])
                .arg(&script)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(dir.path().join(".gitattributes"), "*.txt filter=hostile\n").unwrap();
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();

        assert!(matches!(
            service.git(["add", "--", "a.txt"]),
            Err(GitError::InvalidRepository(message))
                if message.contains("worktree Git configuration")
                    && message.contains("filter.hostile.clean")
        ));
        assert!(!canary.exists());
    }

    #[test]
    fn commit_requires_approved_diff_and_adds_team_trailers() {
        let (dir, service) = repository();
        service.git(["checkout", "-b", "opencoding/task"]).unwrap();
        std::fs::write(dir.path().join("a.txt"), "new\n").unwrap();
        let snapshot = service.diff(&["a.txt".into()], 10000).unwrap();
        assert!(matches!(
            service.commit(CommitRequest {
                message: "change".into(),
                paths: vec!["a.txt".into()],
                expected_diff_hash: "wrong".into(),
                scope: scope(),
                session_id: Id("session".into())
            }),
            Err(GitError::DiffChanged)
        ));
        let result = service
            .commit(CommitRequest {
                message: "change".into(),
                paths: vec!["a.txt".into()],
                expected_diff_hash: snapshot.sha256,
                scope: scope(),
                session_id: Id("session".into()),
            })
            .unwrap();
        assert_eq!(result.branch, "opencoding/task");
        let message = service.text(["show", "-s", "--format=%B", "HEAD"]).unwrap();
        assert!(message.contains("Opencoding-Team: team"));
        assert!(message.contains("Opencoding-Task: task"));
    }

    #[test]
    fn commit_rejects_scope_values_that_can_inject_trailers() {
        for value in [
            "team\nSigned-off-by: attacker",
            "actor\rOpencoding-Team: fake",
            "task\0hidden",
        ] {
            assert!(matches!(
                validate_trailer_value("scope", value),
                Err(GitError::InvalidArgument(message)) if message.contains("single-line")
            ));
        }
    }

    #[test]
    fn commit_includes_selected_untracked_files_without_absorbing_other_staged_changes() {
        let (dir, service) = repository();
        service.git(["checkout", "-b", "opencoding/task"]).unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), "keep staged\n").unwrap();
        service.git(["add", "unrelated.txt"]).unwrap();
        std::fs::write(dir.path().join("a.txt"), "selected tracked\n").unwrap();
        std::fs::write(dir.path().join("selected-new.txt"), "selected untracked\n").unwrap();

        let paths = vec!["a.txt".into(), "selected-new.txt".into()];
        let snapshot = service.diff(&paths, 64 * 1024).unwrap();
        assert!(snapshot.unified_diff.contains("selected untracked"));
        service
            .commit(CommitRequest {
                message: "selected files only".into(),
                paths,
                expected_diff_hash: snapshot.sha256,
                scope: scope(),
                session_id: Id("session".into()),
            })
            .unwrap();

        let committed = service
            .text(["show", "--format=", "--name-only", "HEAD"])
            .unwrap();
        assert!(committed.lines().any(|path| path == "a.txt"));
        assert!(committed.lines().any(|path| path == "selected-new.txt"));
        assert!(!committed.lines().any(|path| path == "unrelated.txt"));
        assert_eq!(
            service.text(["diff", "--cached", "--name-only"]).unwrap(),
            "unrelated.txt\n"
        );
    }

    #[test]
    fn unmanaged_worktree_branches_are_rejected() {
        let (_dir, service) = repository();
        assert!(
            service
                .create_worktree("main", "feature/unmanaged")
                .is_err()
        );
    }

    #[test]
    fn worktree_is_created_on_managed_branch() {
        let (_dir, service) = repository();
        let worktree = service
            .create_worktree("main", "opencoding/isolated")
            .unwrap();
        let path = url::Url::parse(&worktree.workspace_uri)
            .unwrap()
            .to_file_path()
            .unwrap();
        assert!(path.join("a.txt").exists());
        assert_eq!(
            Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(path)
                .output()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .unwrap(),
            "opencoding/isolated"
        );
    }

    #[test]
    fn paths_cannot_escape_repository() {
        let (_dir, service) = repository();
        assert!(service.diff(&["../secret".into()], 100).is_err());
    }

    #[test]
    fn revision_diff_supports_review_ranges_without_option_injection() {
        let (dir, service) = repository();
        std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
        let snapshot = service
            .diff_revision(Some("HEAD"), &["a.txt".into()], 10_000)
            .unwrap();
        assert!(snapshot.unified_diff.contains("+changed"));
        assert!(
            service
                .diff_revision(Some("--output=/tmp/escape"), &[], 10_000)
                .is_err()
        );
        assert!(
            service
                .diff_revision(Some("HEAD;touch-pwned"), &[], 10_000)
                .is_err()
        );
    }
}
