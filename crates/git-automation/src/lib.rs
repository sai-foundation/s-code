use opencoding_protocol::{Id, Scope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
    process::{Command, Output},
};
use thiserror::Error;

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
    #[error("remote branch changed since approval")]
    LeaseChanged,
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
pub struct PushRequest {
    pub remote: String,
    pub branch: String,
    pub expected_remote_oid: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushResult {
    pub remote: String,
    pub branch: String,
    pub oid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorktreeResult {
    pub branch: String,
    pub workspace_uri: String,
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
        let service = Self { root };
        service.git(["rev-parse", "--show-toplevel"])?;
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
        ];
        if let Some(revision) = revision {
            validate_revision(revision)?;
            args.push(revision.into());
        }
        if !paths.is_empty() {
            args.push("--".into());
            args.extend(paths.iter().cloned());
        }
        let bytes = self.git(args)?.stdout;
        let hash = sha256(&bytes);
        let truncated = bytes.len() > max_bytes;
        let selected = &bytes[..bytes.len().min(max_bytes)];
        Ok(DiffSnapshot {
            unified_diff: String::from_utf8_lossy(selected).into_owned(),
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
                let path = self.root.join(relative);
                path.is_file().then_some((relative, path))
            });
        let mut matched_paths = Vec::new();
        let mut codeowners_path = None;
        if let Some((relative, path)) = codeowners {
            let metadata = std::fs::metadata(&path)?;
            if metadata.len() > 1024 * 1024 {
                return Err(GitError::InvalidArgument(
                    "CODEOWNERS exceeds the 1 MiB limit".into(),
                ));
            }
            let contents = std::fs::read_to_string(path)?;
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
        validate_paths(&request.paths)?;
        let branch = self.text(["branch", "--show-current"])?;
        validate_managed_branch(branch.trim())?;
        let current = self.diff(&request.paths, usize::MAX)?;
        if current.sha256 != request.expected_diff_hash {
            return Err(GitError::DiffChanged);
        }
        let mut add = vec!["add".to_owned(), "--".to_owned()];
        add.extend(request.paths.iter().cloned());
        self.git(add)?;
        let staged = self.staged_hash(&request.paths)?;
        if staged != request.expected_diff_hash {
            let mut restore = vec!["reset".to_owned(), "--".to_owned()];
            restore.extend(request.paths.iter().cloned());
            let _ = self.git(restore);
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
        self.git([
            "-c",
            "user.name=Opencoding Agent",
            "-c",
            "user.email=agent@opencoding.local",
            "commit",
            "--no-verify",
            "-m",
            &(request.message + &trailers),
        ])?;
        let status = self.status()?;
        Ok(CommitResult {
            oid: status.head_oid,
            branch: status.branch,
            diff_hash: staged,
        })
    }

    pub fn push(&self, request: PushRequest) -> Result<PushResult, GitError> {
        validate_remote(&request.remote)?;
        validate_managed_branch(&request.branch)?;
        let current = self.status()?;
        if current.branch != request.branch {
            return Err(GitError::InvalidArgument(
                "only the checked-out managed branch may be pushed".into(),
            ));
        }
        let remote_ref = format!("refs/heads/{}", request.branch);
        let advertised = self.git(["ls-remote", "--heads", &request.remote, &remote_ref])?;
        let remote_oid = String::from_utf8_lossy(&advertised.stdout)
            .split_whitespace()
            .next()
            .map(str::to_owned);
        if remote_oid != request.expected_remote_oid {
            return Err(GitError::LeaseChanged);
        }
        let lease = format!(
            "--force-with-lease={remote_ref}:{}",
            request.expected_remote_oid.as_deref().unwrap_or("")
        );
        let refspec = format!("HEAD:{remote_ref}");
        self.git(["push", "--porcelain", &lease, &request.remote, &refspec])?;
        Ok(PushResult {
            remote: request.remote,
            branch: request.branch,
            oid: current.head_oid,
        })
    }

    fn staged_hash(&self, paths: &[String]) -> Result<String, GitError> {
        let mut args = vec![
            "diff".to_owned(),
            "--cached".to_owned(),
            "--binary".to_owned(),
            "--no-ext-diff".to_owned(),
            "--".to_owned(),
        ];
        args.extend(paths.iter().cloned());
        Ok(sha256(&self.git(args)?.stdout))
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
        let output = Command::new("git")
            .args([
                "-c",
                if cfg!(windows) {
                    "core.hooksPath=NUL"
                } else {
                    "core.hooksPath=/dev/null"
                },
                "-c",
                "protocol.file.allow=always",
            ])
            .args(args)
            .current_dir(&self.root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()?;
        if !output.status.success() {
            return Err(GitError::Command(redact_git_error(&output.stderr)));
        }
        Ok(output)
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

fn validate_remote(value: &str) -> Result<(), GitError> {
    if value.is_empty() || value.starts_with('-') || value.contains(char::is_whitespace) {
        return Err(GitError::InvalidArgument("invalid remote name".into()));
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

fn sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn redact_git_error(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() > 4096 {
        format!("{}…", &text[..4096])
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
    fn managed_branch_and_lease_protect_push() {
        let (_dir, service) = repository();
        assert!(
            service
                .create_worktree("main", "feature/unmanaged")
                .is_err()
        );
        assert!(matches!(
            service.push(PushRequest {
                remote: "origin".into(),
                branch: "main".into(),
                expected_remote_oid: None
            }),
            Err(GitError::InvalidArgument(_))
        ));
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
