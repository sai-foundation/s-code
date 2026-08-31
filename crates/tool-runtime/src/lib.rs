use opencoding_platform_runtime::{PlatformRuntime, ProcessOutput, ProcessSpec, RuntimeError};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

const SENSITIVE_COMPONENTS: &[&str] = &[".ssh", ".aws", ".azure", ".gnupg", ".kube"];
const SENSITIVE_FILES: &[&str] = &[".env", ".env.local", "credentials", "id_rsa", "id_ed25519"];

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("path escapes workspace: {0}")]
    Boundary(String),
    #[error("sensitive path is denied: {0}")]
    Sensitive(String),
    #[error("file changed since it was read")]
    ConcurrentModification,
    #[error("expected hash is required when replacing an existing file")]
    MissingExpectedHash,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("runtime error: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("invalid argument: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub sha256: String,
    pub revision: String,
    pub total_lines: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileReplacement {
    pub path: String,
    #[serde(deserialize_with = "deserialize_expected_sha256")]
    pub expected_sha256: Option<String>,
    pub content: String,
}

fn deserialize_expected_sha256<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.filter(|value| !value.eq_ignore_ascii_case("null")))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WriteResult {
    pub path: String,
    pub previous_sha256: Option<String>,
    pub sha256: String,
    pub bytes_written: usize,
}

#[derive(Clone, Debug)]
pub struct FileSnapshot {
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub kind: String,
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileListing {
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

pub struct ToolRuntime {
    root: PathBuf,
    root_uri: String,
    platform: Arc<dyn PlatformRuntime>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CommandCompatibility {
    pub workspace_writable: bool,
    pub browser_compatible: bool,
}

impl ToolRuntime {
    pub fn open(
        workspace_uri: &str,
        platform: Arc<dyn PlatformRuntime>,
    ) -> Result<Self, ToolError> {
        let root = platform.canonicalize_workspace(workspace_uri)?;
        if !root.is_dir() {
            return Err(ToolError::Invalid("workspace is not a directory".into()));
        }
        let root_uri = url::Url::from_directory_path(&root)
            .map_err(|_| ToolError::Invalid("workspace URI".into()))?
            .to_string();
        Ok(Self {
            root,
            root_uri,
            platform,
        })
    }

    pub fn read_file(
        &self,
        relative: &str,
        start_line: usize,
        end_line: usize,
        max_bytes: usize,
    ) -> Result<FileContent, ToolError> {
        if start_line == 0 || end_line < start_line {
            return Err(ToolError::Invalid(
                "line range must be one-based and ordered".into(),
            ));
        }
        let path = self.resolve(relative, false)?;
        let bytes = fs::read(&path)?;
        let full_hash = sha256(&bytes);
        let text = String::from_utf8_lossy(&bytes);
        let total_lines = text.lines().count();
        let selected = text
            .lines()
            .skip(start_line - 1)
            .take(end_line - start_line + 1)
            .collect::<Vec<_>>()
            .join("\n");
        let byte_limited = selected.len() > max_bytes;
        let content = if byte_limited {
            String::from_utf8_lossy(&selected.as_bytes()[..max_bytes]).into_owned()
        } else {
            selected
        };
        Ok(FileContent {
            path: relative.into(),
            content,
            revision: full_hash[..16].into(),
            sha256: full_hash,
            total_lines,
            truncated: byte_limited || end_line < total_lines,
        })
    }

    pub fn list_files(
        &self,
        relative: &str,
        depth: usize,
        limit: usize,
    ) -> Result<FileListing, ToolError> {
        let start = if relative.is_empty() || relative == "." {
            self.root.clone()
        } else {
            self.resolve(relative, false)?
        };
        if !start.is_dir() {
            return Err(ToolError::Invalid("list path is not a directory".into()));
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        let walker = ignore::WalkBuilder::new(&start)
            .max_depth(Some(depth.saturating_add(1)))
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .add_custom_ignore_filename(".gitignore")
            .parents(true)
            .follow_links(false)
            .build();
        for item in walker.skip(1) {
            let entry = item.map_err(|error| ToolError::Io(std::io::Error::other(error)))?;
            let path = entry.path();
            let relative_path = path
                .strip_prefix(&self.root)
                .map_err(|_| ToolError::Boundary(path.display().to_string()))?;
            if relative_path
                .components()
                .any(|part| part.as_os_str() == ".git")
            {
                continue;
            }
            if self.reject_sensitive(relative_path).is_err() {
                continue;
            }
            if entries.len() == limit {
                truncated = true;
                break;
            }
            let metadata = entry
                .metadata()
                .map_err(|error| ToolError::Io(std::io::Error::other(error)))?;
            entries.push(FileEntry {
                path: relative_path.to_string_lossy().replace('\\', "/"),
                kind: if metadata.is_dir() {
                    "directory"
                } else if metadata.is_file() {
                    "file"
                } else {
                    "other"
                }
                .into(),
                bytes: metadata.is_file().then_some(metadata.len()),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(FileListing { entries, truncated })
    }

    pub fn apply_replacement(
        &self,
        replacement: FileReplacement,
    ) -> Result<WriteResult, ToolError> {
        let path = self.resolve(&replacement.path, true)?;
        let (previous_sha256, permissions) = if path.exists() {
            let current = fs::read(&path)?;
            let hash = sha256(&current);
            let expected = replacement
                .expected_sha256
                .as_ref()
                .ok_or(ToolError::MissingExpectedHash)?;
            if expected != &hash {
                return Err(ToolError::ConcurrentModification);
            }
            (Some(hash), Some(fs::metadata(&path)?.permissions()))
        } else {
            if replacement.expected_sha256.is_some() {
                return Err(ToolError::ConcurrentModification);
            }
            (None, None)
        };
        let parent = path
            .parent()
            .ok_or_else(|| ToolError::Boundary(replacement.path.clone()))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(replacement.content.as_bytes())?;
        temp.as_file().sync_all()?;
        if let Some(permissions) = permissions {
            temp.as_file().set_permissions(permissions)?;
        }
        temp.persist(&path).map_err(|e| ToolError::Io(e.error))?;
        Ok(WriteResult {
            path: replacement.path,
            previous_sha256,
            sha256: sha256(replacement.content.as_bytes()),
            bytes_written: replacement.content.len(),
        })
    }

    pub fn snapshot_file(&self, relative: &str) -> Result<FileSnapshot, ToolError> {
        let path = self.resolve(relative, true)?;
        if !path.exists() {
            return Ok(FileSnapshot {
                path: relative.into(),
                content: None,
                sha256: None,
            });
        }
        if !path.is_file() {
            return Err(ToolError::Invalid("snapshot path is not a file".into()));
        }
        let content = fs::read(path)?;
        Ok(FileSnapshot {
            path: relative.into(),
            sha256: Some(sha256(&content)),
            content: Some(content),
        })
    }

    pub fn verify_file_hash(&self, relative: &str, expected: &str) -> Result<(), ToolError> {
        let snapshot = self.snapshot_file(relative)?;
        if snapshot.sha256.as_deref() != Some(expected) {
            return Err(ToolError::ConcurrentModification);
        }
        Ok(())
    }

    pub fn restore_file(
        &self,
        relative: &str,
        expected_current_sha256: &str,
        before_content: Option<&[u8]>,
    ) -> Result<(), ToolError> {
        self.verify_file_hash(relative, expected_current_sha256)?;
        let path = self.resolve(relative, false)?;
        if let Some(content) = before_content {
            let parent = path
                .parent()
                .ok_or_else(|| ToolError::Boundary(relative.into()))?;
            let permissions = fs::metadata(&path)?.permissions();
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            temp.write_all(content)?;
            temp.as_file().sync_all()?;
            temp.as_file().set_permissions(permissions)?;
            temp.persist(&path)
                .map_err(|error| ToolError::Io(error.error))?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub async fn search_text(
        &self,
        query: &str,
        glob: Option<&str>,
        limit_bytes: usize,
    ) -> Result<ProcessOutput, ToolError> {
        if query.is_empty() {
            return Err(ToolError::Invalid("search query is empty".into()));
        }
        let mut args = vec![
            "--line-number".into(),
            "--no-heading".into(),
            "--color=never".into(),
            "--hidden".into(),
            "--glob=!.git/**".into(),
        ];
        if let Some(glob) = glob {
            args.push(format!("--glob={glob}"));
        }
        args.push("--".into());
        args.push(query.into());
        args.push(".".into());
        self.run("rg", args, Duration::from_secs(30), false, limit_bytes)
            .await
    }

    pub async fn run(
        &self,
        program: &str,
        args: Vec<String>,
        timeout: Duration,
        network_enabled: bool,
        output_limit_bytes: usize,
    ) -> Result<ProcessOutput, ToolError> {
        self.run_with_profile(
            program,
            args,
            timeout,
            network_enabled,
            output_limit_bytes,
            true,
        )
        .await
    }

    pub async fn run_with_profile(
        &self,
        program: &str,
        args: Vec<String>,
        timeout: Duration,
        network_enabled: bool,
        output_limit_bytes: usize,
        workspace_writable: bool,
    ) -> Result<ProcessOutput, ToolError> {
        self.run_with_compatibility(
            program,
            args,
            timeout,
            network_enabled,
            output_limit_bytes,
            CommandCompatibility {
                workspace_writable,
                browser_compatible: false,
            },
        )
        .await
    }

    pub async fn run_with_compatibility(
        &self,
        program: &str,
        args: Vec<String>,
        timeout: Duration,
        network_enabled: bool,
        output_limit_bytes: usize,
        compatibility: CommandCompatibility,
    ) -> Result<ProcessOutput, ToolError> {
        if program.contains('/') || program.contains('\\') {
            return Err(ToolError::Invalid(
                "program must be resolved through the sandbox PATH".into(),
            ));
        }
        let executable = find_program(program)
            .ok_or_else(|| ToolError::Invalid(format!("program not found on PATH: {program}")))?;
        let interpreter = shebang_interpreter(&executable);
        let command_program = interpreter.as_ref().unwrap_or(&executable);
        let mut command_args = args;
        if interpreter.is_some() {
            command_args.insert(0, executable.canonicalize()?.to_string_lossy().into_owned());
        }
        let mut readable_root_uris = vec![self.root_uri.clone()];
        let runtime_roots = executable_runtime_roots(&executable)
            .into_iter()
            .chain(
                interpreter
                    .as_deref()
                    .into_iter()
                    .flat_map(executable_runtime_roots),
            )
            .collect::<Vec<_>>();
        for root in runtime_roots {
            let uri = url::Url::from_directory_path(&root)
                .map_err(|_| ToolError::Invalid("executable URI".into()))?
                .to_string();
            if !readable_root_uris.contains(&uri) {
                readable_root_uris.push(uri);
            }
        }
        let scratch = tempfile::Builder::new()
            .prefix(".opencoding-command-")
            .tempdir_in(&self.root)?;
        let scratch_uri = url::Url::from_directory_path(scratch.path())
            .map_err(|_| ToolError::Invalid("command scratch URI".into()))?
            .to_string();
        let mut environment_handles = BTreeMap::new();
        environment_handles.insert(
            "TMPDIR".into(),
            scratch.path().to_string_lossy().into_owned(),
        );
        if let Some(browsers) =
            workspace_playwright_browsers(&self.root, std::env::var_os("PLAYWRIGHT_BROWSERS_PATH"))
        {
            environment_handles.insert(
                "PLAYWRIGHT_BROWSERS_PATH".into(),
                browsers.to_string_lossy().into_owned(),
            );
        }
        let mut writable_root_uris = vec![scratch_uri];
        if compatibility.workspace_writable {
            writable_root_uris.push(self.root_uri.clone());
        }
        let output = self
            .platform
            .execute(ProcessSpec {
                program: command_program.to_string_lossy().into_owned(),
                args: command_args,
                cwd_uri: self.root_uri.clone(),
                environment_handles,
                timeout,
                network_enabled,
                browser_compatible: compatibility.browser_compatible,
                readable_root_uris,
                writable_root_uris,
                output_limit_bytes,
            })
            .await?;
        drop(scratch);
        Ok(output)
    }

    fn resolve(&self, relative: &str, allow_missing: bool) -> Result<PathBuf, ToolError> {
        let input = Path::new(relative);
        if input.as_os_str().is_empty()
            || input.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ToolError::Boundary(relative.into()));
        }
        self.reject_sensitive(input)?;
        let joined = self.root.join(input);
        let checked = if joined.exists() {
            joined.canonicalize()?
        } else if allow_missing {
            let parent = joined
                .parent()
                .ok_or_else(|| ToolError::Boundary(relative.into()))?
                .canonicalize()?;
            parent.join(
                joined
                    .file_name()
                    .ok_or_else(|| ToolError::Boundary(relative.into()))?,
            )
        } else {
            return Err(ToolError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                relative,
            )));
        };
        if !checked.starts_with(&self.root) {
            return Err(ToolError::Boundary(relative.into()));
        }
        Ok(checked)
    }

    fn reject_sensitive(&self, path: &Path) -> Result<(), ToolError> {
        for component in path.components().filter_map(|c| c.as_os_str().to_str()) {
            let lower = component.to_ascii_lowercase();
            if SENSITIVE_COMPONENTS.contains(&lower.as_str())
                || SENSITIVE_FILES.contains(&lower.as_str())
            {
                return Err(ToolError::Sensitive(path.display().to_string()));
            }
        }
        Ok(())
    }
}

fn workspace_playwright_browsers(
    workspace: &Path,
    configured: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let workspace = workspace.canonicalize().ok()?;
    let configured = PathBuf::from(configured?);
    let configured = configured.canonicalize().ok()?;
    configured
        .is_dir()
        .then_some(configured)
        .filter(|configured| configured.starts_with(&workspace))
}

fn find_program(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    find_program_in(
        program,
        std::env::split_paths(&paths),
        &platform_executable_extensions(),
    )
}

fn executable_runtime_roots(executable: &Path) -> Vec<PathBuf> {
    let mut roots = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    if let Ok(canonical) = executable.canonicalize()
        && canonical != executable
        && let Some(parent) = canonical.parent()
    {
        roots.push(parent.to_path_buf());
        if let Some(prefix) = parent.parent() {
            roots.push(prefix.to_path_buf());
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn shebang_interpreter(executable: &Path) -> Option<PathBuf> {
    let canonical = executable.canonicalize().ok()?;
    let content = fs::read(canonical).ok()?;
    let first_line = content.split(|byte| *byte == b'\n').next()?;
    let shebang = std::str::from_utf8(first_line).ok()?.strip_prefix("#!")?;
    let mut parts = shebang.split_whitespace();
    let interpreter = parts.next()?;
    if interpreter == "/usr/bin/env" {
        return parts.next().and_then(find_program);
    }
    let interpreter = PathBuf::from(interpreter);
    interpreter.is_file().then_some(interpreter)
}

fn find_program_in(
    program: &str,
    paths: impl IntoIterator<Item = PathBuf>,
    executable_extensions: &[std::ffi::OsString],
) -> Option<PathBuf> {
    for path in paths {
        let candidate = path.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        for extension in executable_extensions {
            let mut executable = std::ffi::OsString::from(program);
            executable.push(extension);
            let candidate = path.join(executable);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn platform_executable_extensions() -> Vec<std::ffi::OsString> {
    let mut extensions = std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter_map(|extension| {
                    let extension = extension.trim();
                    (!extension.is_empty()
                        && extension.starts_with('.')
                        && !extension.contains('/')
                        && !extension.contains('\\'))
                    .then(|| std::ffi::OsString::from(extension))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if extensions.is_empty() {
        extensions = [".COM", ".EXE", ".BAT", ".CMD"]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect();
    }
    extensions
}

#[cfg(not(windows))]
fn platform_executable_extensions() -> Vec<std::ffi::OsString> {
    Vec::new()
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn content_sha256(bytes: &[u8]) -> String {
    sha256(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use opencoding_protocol::Capability;
    use std::sync::Mutex;
    struct TestRuntime;
    #[async_trait]
    impl PlatformRuntime for TestRuntime {
        fn capabilities(&self) -> Vec<Capability> {
            vec![]
        }
        fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError> {
            url::Url::parse(uri)
                .unwrap()
                .to_file_path()
                .unwrap()
                .canonicalize()
                .map_err(|e| RuntimeError::InvalidBoundary(e.to_string()))
        }
        async fn execute(&self, spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
            Ok(ProcessOutput {
                exit_code: Some(0),
                stdout: format!("{} {:?}", spec.program, spec.args),
                stderr: String::new(),
                truncated: false,
            })
        }
        async fn cancel_process_tree(&self, _: &str) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    struct CapturingRuntime {
        specifications: Arc<Mutex<Vec<ProcessSpec>>>,
    }

    #[async_trait]
    impl PlatformRuntime for CapturingRuntime {
        fn capabilities(&self) -> Vec<Capability> {
            vec![]
        }

        fn canonicalize_workspace(&self, uri: &str) -> Result<PathBuf, RuntimeError> {
            url::Url::parse(uri)
                .unwrap()
                .to_file_path()
                .unwrap()
                .canonicalize()
                .map_err(|error| RuntimeError::InvalidBoundary(error.to_string()))
        }

        async fn execute(&self, spec: ProcessSpec) -> Result<ProcessOutput, RuntimeError> {
            self.specifications.lock().unwrap().push(spec);
            Ok(ProcessOutput {
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                truncated: false,
            })
        }

        async fn cancel_process_tree(&self, _: &str) -> Result<(), RuntimeError> {
            Ok(())
        }
    }

    fn runtime() -> (tempfile::TempDir, ToolRuntime) {
        let dir = tempfile::tempdir().unwrap();
        let uri = url::Url::from_directory_path(dir.path())
            .unwrap()
            .to_string();
        let runtime = ToolRuntime::open(&uri, Arc::new(TestRuntime)).unwrap();
        (dir, runtime)
    }

    #[test]
    fn blocks_parent_traversal_and_secrets() {
        let (_dir, runtime) = runtime();
        assert!(matches!(
            runtime.read_file("../secret", 1, 1, 100),
            Err(ToolError::Boundary(_))
        ));
        assert!(matches!(
            runtime.apply_replacement(FileReplacement {
                path: ".env".into(),
                expected_sha256: None,
                content: "x".into()
            }),
            Err(ToolError::Sensitive(_))
        ));
    }
    #[test]
    fn replacement_requires_matching_hash() {
        let (dir, runtime) = runtime();
        fs::write(dir.path().join("a.txt"), "old").unwrap();
        let read = runtime.read_file("a.txt", 1, 10, 100).unwrap();
        assert!(matches!(
            runtime.apply_replacement(FileReplacement {
                path: "a.txt".into(),
                expected_sha256: Some("bad".into()),
                content: "new".into()
            }),
            Err(ToolError::ConcurrentModification)
        ));
        runtime
            .apply_replacement(FileReplacement {
                path: "a.txt".into(),
                expected_sha256: Some(read.sha256),
                content: "new".into(),
            })
            .unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "new");
    }

    #[test]
    fn replacement_accepts_string_null_from_compatible_tool_callers() {
        let replacement: FileReplacement = serde_json::from_value(serde_json::json!({
            "path": "new.txt",
            "expected_sha256": "null",
            "content": "new"
        }))
        .unwrap();

        assert_eq!(replacement.expected_sha256, None);
    }

    #[test]
    fn read_file_reports_truncated_line_ranges() {
        let (dir, runtime) = runtime();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();

        let partial = runtime.read_file("a.txt", 1, 2, 100).unwrap();
        assert_eq!(partial.content, "one\ntwo");
        assert_eq!(partial.total_lines, 3);
        assert!(partial.truncated);

        let complete = runtime.read_file("a.txt", 1, 3, 100).unwrap();
        assert!(!complete.truncated);
    }

    #[test]
    fn snapshots_restore_replaced_and_created_files_with_hash_guard() {
        let (dir, runtime) = runtime();
        fs::write(dir.path().join("existing.txt"), "before").unwrap();
        let before = runtime.snapshot_file("existing.txt").unwrap();
        let written = runtime
            .apply_replacement(FileReplacement {
                path: "existing.txt".into(),
                expected_sha256: before.sha256.clone(),
                content: "after".into(),
            })
            .unwrap();
        runtime
            .restore_file("existing.txt", &written.sha256, before.content.as_deref())
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("existing.txt")).unwrap(),
            "before"
        );

        let created = runtime
            .apply_replacement(FileReplacement {
                path: "created.txt".into(),
                expected_sha256: None,
                content: "created".into(),
            })
            .unwrap();
        fs::write(dir.path().join("created.txt"), "user changed it").unwrap();
        assert!(matches!(
            runtime.restore_file("created.txt", &created.sha256, None),
            Err(ToolError::ConcurrentModification)
        ));
        assert!(dir.path().join("created.txt").exists());
    }

    #[test]
    fn listing_respects_gitignore_and_sensitive_paths() {
        let (dir, runtime) = runtime();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("visible.txt"), "ok").unwrap();
        fs::write(dir.path().join("ignored.txt"), "no").unwrap();
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        fs::write(dir.path().join(".ssh/key"), "no").unwrap();
        let listing = runtime.list_files(".", 4, 100).unwrap();
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.path == "visible.txt")
        );
        assert!(
            !listing
                .entries
                .iter()
                .any(|entry| entry.path.contains("ignored.txt"))
        );
        assert!(
            !listing
                .entries
                .iter()
                .any(|entry| entry.path.contains(".ssh"))
        );
    }

    #[test]
    fn program_resolution_supports_platform_executable_extensions() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("cargo.exe");
        fs::write(&executable, "fixture").unwrap();

        assert_eq!(
            find_program_in(
                "cargo",
                [directory.path().to_path_buf()],
                &[std::ffi::OsString::from(".exe")],
            ),
            Some(executable)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_executable_includes_its_runtime_prefix() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let prefix = directory.path().join("Cellar/node/1.0");
        let runtime_bin = prefix.join("bin");
        let public_bin = directory.path().join("bin");
        fs::create_dir_all(&runtime_bin).unwrap();
        fs::create_dir_all(&public_bin).unwrap();
        let runtime = runtime_bin.join("node");
        fs::write(&runtime, "fixture").unwrap();
        let public = public_bin.join("node");
        symlink(&runtime, &public).unwrap();

        let roots = executable_runtime_roots(&public);
        assert!(roots.contains(&public_bin));
        assert!(roots.contains(&runtime_bin.canonicalize().unwrap()));
        assert!(roots.contains(&prefix.canonicalize().unwrap()));
    }

    #[tokio::test]
    async fn commands_are_structured() {
        let (_dir, runtime) = runtime();
        let output = runtime
            .run(
                "cargo",
                vec!["test".into()],
                Duration::from_secs(1),
                false,
                100,
            )
            .await
            .unwrap();
        assert!(output.stdout.contains("cargo"));
        assert!(
            runtime
                .run("/bin/sh", vec![], Duration::from_secs(1), false, 100)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn command_profiles_control_the_workspace_write_mount() {
        let directory = tempfile::tempdir().unwrap();
        let root_uri = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let specifications = Arc::new(Mutex::new(Vec::new()));
        let runtime = ToolRuntime::open(
            &root_uri,
            Arc::new(CapturingRuntime {
                specifications: specifications.clone(),
            }),
        )
        .unwrap();
        let workspace_root_uri = runtime.root_uri.clone();

        runtime
            .run_with_profile(
                "cargo",
                vec!["check".into()],
                Duration::from_secs(1),
                false,
                1024,
                false,
            )
            .await
            .unwrap();
        runtime
            .run_with_profile(
                "cargo",
                vec!["fmt".into()],
                Duration::from_secs(1),
                false,
                1024,
                true,
            )
            .await
            .unwrap();
        runtime
            .run_with_compatibility(
                "cargo",
                vec!["test".into()],
                Duration::from_secs(1),
                false,
                1024,
                CommandCompatibility {
                    workspace_writable: true,
                    browser_compatible: true,
                },
            )
            .await
            .unwrap();

        let specifications = specifications.lock().unwrap();
        assert_eq!(specifications[0].writable_root_uris.len(), 1);
        assert_eq!(specifications[1].writable_root_uris.len(), 2);
        assert_eq!(specifications[1].writable_root_uris[1], workspace_root_uri);
        assert!(!specifications[0].browser_compatible);
        assert!(!specifications[1].browser_compatible);
        assert!(specifications[2].browser_compatible);
        for specification in specifications.iter() {
            assert!(specification.environment_handles.contains_key("TMPDIR"));
        }
    }

    #[test]
    fn playwright_browser_path_is_forwarded_only_from_inside_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let browsers = workspace.path().join("playwright-browsers");
        fs::create_dir(&browsers).unwrap();
        assert_eq!(
            workspace_playwright_browsers(workspace.path(), Some(browsers.as_os_str().to_owned())),
            Some(browsers.canonicalize().unwrap())
        );

        let external = tempfile::tempdir().unwrap();
        assert_eq!(
            workspace_playwright_browsers(
                workspace.path(),
                Some(external.path().as_os_str().to_owned())
            ),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn native_search_runs_inside_seatbelt() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("source.rs"), "fn needle() {}\n").unwrap();
        let uri = url::Url::from_directory_path(dir.path())
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();
        let output = runtime
            .search_text("needle", Some("*.rs"), 4096)
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert!(output.stdout.contains("source.rs:1"), "{output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn symlinked_node_runtime_runs_inside_seatbelt_when_installed() {
        if find_program("node").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let uri = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();

        let output = runtime
            .run_with_profile(
                "node",
                vec!["--version".into()],
                Duration::from_secs(5),
                false,
                4096,
                false,
            )
            .await
            .unwrap();

        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert!(output.stdout.starts_with('v'), "{output:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn env_shebang_runtime_runs_inside_seatbelt_when_installed() {
        if find_program("npm").is_none() || find_program("node").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let uri = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();

        let output = runtime
            .run_with_profile(
                "npm",
                vec!["--version".into()],
                Duration::from_secs(10),
                false,
                4096,
                false,
            )
            .await
            .unwrap();

        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert!(!output.stdout.trim().is_empty(), "{output:?}");
    }
}
