use opencoding_platform_runtime::{PlatformRuntime, ProcessOutput, ProcessSpec, RuntimeError};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};
#[cfg(unix)]
use std::{
    ffi::OsString,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

const SENSITIVE_COMPONENTS: &[&str] = &[
    ".opencoding",
    ".ssh",
    ".aws",
    ".azure",
    ".docker",
    ".gnupg",
    ".kube",
    ".password-store",
    ".terraform.d",
];
const SENSITIVE_FILES: &[&str] = &[
    ".env",
    ".env.local",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".openrouter_apikey",
    ".pypirc",
    "credentials",
    "credentials.json",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ed25519_sk",
    "id_rsa",
    "key.json",
    "secrets.json",
    "service-account.json",
];
const MAX_TOOL_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_READ_RESPONSE_BYTES: usize = 1024 * 1024;
#[cfg(unix)]
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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
    #[cfg(unix)]
    root_directory: Arc<fs::File>,
    root_uri: String,
    platform: Arc<dyn PlatformRuntime>,
    dependency_cache_base: PathBuf,
    runtime_home: Option<PathBuf>,
    program_search_path: Vec<PathBuf>,
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
        #[cfg(unix)]
        let root_directory = Arc::new(open_directory_no_follow(&root)?);
        Ok(Self {
            root,
            #[cfg(unix)]
            root_directory,
            root_uri,
            platform,
            dependency_cache_base: tool_dependency_cache_base()?,
            runtime_home: environment_home_directory(|name| std::env::var_os(name), cfg!(windows))
                .and_then(|path| path.canonicalize().ok()),
            program_search_path: std::env::var_os("PATH")
                .map(|paths| {
                    std::env::split_paths(&paths)
                        .filter(|path| path.is_absolute())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    #[cfg(all(test, unix))]
    fn with_dependency_cache_base(mut self, path: PathBuf) -> Self {
        self.dependency_cache_base = path;
        self
    }

    #[cfg(all(test, unix))]
    fn with_runtime_environment(mut self, home: PathBuf, search_path: Vec<PathBuf>) -> Self {
        self.runtime_home = Some(home);
        self.program_search_path = search_path;
        self
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
        if max_bytes == 0 {
            return Err(ToolError::Invalid(
                "read response limit must be positive".into(),
            ));
        }
        let max_bytes = max_bytes.min(MAX_READ_RESPONSE_BYTES);
        let bytes = self.read_workspace_file(relative)?;
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
            let mut end = max_bytes;
            while !selected.is_char_boundary(end) {
                end -= 1;
            }
            selected[..end].to_owned()
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
        #[cfg(unix)]
        {
            self.secure_list_files(relative, depth, limit)
        }
        #[cfg(not(unix))]
        {
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
    }

    pub fn apply_replacement(
        &self,
        replacement: FileReplacement,
    ) -> Result<WriteResult, ToolError> {
        if replacement.content.len() as u64 > MAX_TOOL_FILE_BYTES {
            return Err(ToolError::Invalid(
                "replacement exceeds the 16 MiB file-tool limit".into(),
            ));
        }
        let previous_sha256 = self.replace_workspace_file(
            &replacement.path,
            replacement.expected_sha256.as_deref(),
            replacement.content.as_bytes(),
        )?;
        Ok(WriteResult {
            path: replacement.path,
            previous_sha256,
            sha256: sha256(replacement.content.as_bytes()),
            bytes_written: replacement.content.len(),
        })
    }

    pub fn snapshot_file(&self, relative: &str) -> Result<FileSnapshot, ToolError> {
        let Some(content) = self.read_optional_workspace_file(relative)? else {
            return Ok(FileSnapshot {
                path: relative.into(),
                content: None,
                sha256: None,
            });
        };
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
        if let Some(content) = before_content {
            if content.len() as u64 > MAX_TOOL_FILE_BYTES {
                return Err(ToolError::Invalid(
                    "restore image exceeds the 16 MiB file-tool limit".into(),
                ));
            }
            self.replace_workspace_file(relative, Some(expected_current_sha256), content)?;
        } else {
            self.remove_workspace_file(relative, expected_current_sha256)?;
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
        self.run_with_profile(
            "rg",
            args,
            Duration::from_secs(30),
            false,
            limit_bytes,
            false,
        )
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
        self.ensure_authorized_root()?;
        let output_limit_bytes = output_limit_bytes.clamp(1, MAX_READ_RESPONSE_BYTES);
        if program.contains('/') || program.contains('\\') {
            return Err(ToolError::Invalid(
                "program must be resolved through the sandbox PATH".into(),
            ));
        }
        let executable = find_program_in(
            program,
            self.program_search_path.iter().cloned(),
            &platform_executable_extensions(),
        )
        .ok_or_else(|| ToolError::Invalid(format!("program not found on PATH: {program}")))?;
        if !executable.is_absolute() || executable.canonicalize()?.starts_with(&self.root) {
            return Err(ToolError::Invalid(
                "programs resolved from inside the workspace are not trusted executables".into(),
            ));
        }
        let interpreter = shebang_interpreter(&executable, &self.program_search_path);
        if let Some(interpreter) = &interpreter
            && (!interpreter.is_absolute() || interpreter.canonicalize()?.starts_with(&self.root))
        {
            return Err(ToolError::Invalid(
                "a workspace or relative shebang interpreter is not trusted".into(),
            ));
        }
        let scratch = tempfile::Builder::new()
            .prefix(".opencoding-command-")
            .tempdir_in(&self.root)?;
        // On macOS, keep native binaries at their canonical install path so
        // loader-relative libraries (for example Homebrew's libnode) resolve
        // normally. Scripts are still copied into the private scratch area.
        // Linux bind-mounts the staged binary because arbitrary host paths are
        // intentionally absent from its filesystem view.
        let canonical_executable = executable.canonicalize()?;
        let rustup_multicall = executable.file_name() != canonical_executable.file_name()
            && matches!(
                canonical_executable
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("rustup" | "rustup-init")
            );
        let managed_program =
            managed_package_runtime_root(&executable, self.runtime_home.as_deref()).is_some();
        let managed_shim = managed_home_shim(&executable, self.runtime_home.as_deref()).is_some();
        let staged_program = if managed_shim {
            executable.clone()
        } else if (interpreter.is_some() && !managed_program)
            || rustup_multicall
            || (!cfg!(target_os = "macos") && !managed_program)
        {
            stage_executable(&executable, scratch.path(), "program")?
        } else {
            canonical_executable.clone()
        };
        if rustup_multicall {
            stage_rustup_companions(&canonical_executable, &staged_program)?;
        }
        let staged_interpreter = interpreter
            .as_deref()
            .map(|path| {
                if cfg!(target_os = "macos") {
                    path.canonicalize().map_err(ToolError::from)
                } else {
                    stage_executable(path, scratch.path(), "interpreter")
                }
            })
            .transpose()?;
        let command_program = staged_interpreter.as_ref().unwrap_or(&staged_program);
        let mut command_args = args;
        if interpreter.is_some() {
            command_args.insert(0, staged_program.to_string_lossy().into_owned());
        }
        let mut readable_root_uris = vec![self.root_uri.clone()];
        let scratch_uri = url::Url::from_directory_path(scratch.path())
            .map_err(|_| ToolError::Invalid("command scratch URI".into()))?
            .to_string();
        let mut environment_handles = BTreeMap::new();
        environment_handles.insert(
            "TMPDIR".into(),
            scratch.path().to_string_lossy().into_owned(),
        );
        let mut runtime_support = build_runtime_support(
            &executable,
            scratch.path(),
            &self.root,
            &self.dependency_cache_base,
            self.runtime_home.as_deref(),
        )?;
        if let Some(root) = interpreter
            .as_deref()
            .and_then(|path| managed_package_runtime_root(path, self.runtime_home.as_deref()))
            && !runtime_support.readable_roots.contains(&root)
        {
            runtime_support.readable_roots.push(root);
        }
        for root in runtime_support.readable_roots {
            let uri = if root.is_dir() {
                url::Url::from_directory_path(&root)
            } else {
                url::Url::from_file_path(&root)
            }
            .map_err(|_| ToolError::Invalid("build runtime URI".into()))?
            .to_string();
            if !readable_root_uris.contains(&uri) {
                readable_root_uris.push(uri);
            }
        }
        environment_handles.extend(runtime_support.environment);
        if let Some(browsers) =
            workspace_playwright_browsers(&self.root, std::env::var_os("PLAYWRIGHT_BROWSERS_PATH"))
        {
            environment_handles.insert(
                "PLAYWRIGHT_BROWSERS_PATH".into(),
                browsers.to_string_lossy().into_owned(),
            );
        }
        let mut writable_root_uris = vec![scratch_uri];
        for root in runtime_support.writable_roots {
            if root.starts_with(&self.root) {
                return Err(ToolError::Invalid(
                    "the dependency cache must be outside the workspace".into(),
                ));
            }
            let uri = url::Url::from_directory_path(&root)
                .map_err(|_| ToolError::Invalid("dependency cache URI".into()))?
                .to_string();
            if !readable_root_uris.contains(&uri) {
                readable_root_uris.push(uri.clone());
            }
            if !writable_root_uris.contains(&uri) {
                writable_root_uris.push(uri);
            }
        }
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
                denied_read_uris: self.sensitive_workspace_uris()?,
                output_limit_bytes,
                #[cfg(unix)]
                pinned_cwd: Some(self.root_directory.clone()),
            })
            .await?;
        drop(scratch);
        Ok(output)
    }

    #[cfg(unix)]
    fn ensure_authorized_root(&self) -> Result<(), ToolError> {
        use std::os::unix::fs::MetadataExt;

        let opened = self.root_directory.metadata()?;
        let named = self.root.metadata()?;
        if opened.dev() != named.dev() || opened.ino() != named.ino() {
            return Err(ToolError::Boundary(
                "workspace path no longer names the authorized directory".into(),
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn ensure_authorized_root(&self) -> Result<(), ToolError> {
        if self.root.canonicalize()? != self.root {
            return Err(ToolError::Boundary(
                "workspace path no longer names the authorized directory".into(),
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
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
        let canonical_relative = checked
            .strip_prefix(&self.root)
            .map_err(|_| ToolError::Boundary(relative.into()))?;
        self.reject_sensitive(canonical_relative)?;
        Ok(checked)
    }

    #[cfg(unix)]
    fn validated_components(&self, relative: &str) -> Result<Vec<OsString>, ToolError> {
        let input = Path::new(relative);
        if input.as_os_str().is_empty()
            || input.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(ToolError::Boundary(relative.into()));
        }
        self.reject_sensitive(input)?;
        let components = input
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_owned()),
                Component::CurDir => None,
                _ => None,
            })
            .collect::<Vec<_>>();
        if components.is_empty() {
            return Err(ToolError::Boundary(relative.into()));
        }
        Ok(components)
    }

    #[cfg(unix)]
    fn secure_parent(&self, relative: &str) -> Result<(fs::File, OsString), ToolError> {
        use rustix::fs::{Mode, OFlags, openat};

        let components = self.validated_components(relative)?;
        let (leaf, parents) = components
            .split_last()
            .ok_or_else(|| ToolError::Boundary(relative.into()))?;
        let mut directory = self.root_directory.try_clone()?;
        for component in parents {
            let descriptor = openat(
                &directory,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|error| secure_path_error(relative, error.into()))?;
            directory = descriptor.into();
        }
        Ok((directory, leaf.clone()))
    }

    #[cfg(unix)]
    fn secure_open_file(&self, relative: &str) -> Result<fs::File, ToolError> {
        use rustix::fs::{Mode, OFlags, openat};

        let (parent, leaf) = self.secure_parent(relative)?;
        let descriptor = openat(
            &parent,
            &leaf,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|error| secure_path_error(relative, error.into()))?;
        Ok(descriptor.into())
    }

    fn read_workspace_file(&self, relative: &str) -> Result<Vec<u8>, ToolError> {
        #[cfg(unix)]
        {
            read_bounded_regular_handle(self.secure_open_file(relative)?)
        }
        #[cfg(not(unix))]
        {
            let path = self.resolve(relative, false)?;
            read_bounded_regular_file(&path)
        }
    }

    fn read_optional_workspace_file(&self, relative: &str) -> Result<Option<Vec<u8>>, ToolError> {
        match self.read_workspace_file(relative) {
            Ok(content) => Ok(Some(content)),
            Err(ToolError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    fn secure_list_files(
        &self,
        relative: &str,
        depth: usize,
        limit: usize,
    ) -> Result<FileListing, ToolError> {
        use rustix::fs::{Mode, OFlags, openat};

        let start_relative = if relative.is_empty() || relative == "." {
            PathBuf::new()
        } else {
            self.validated_components(relative)?.into_iter().collect()
        };
        let mut start = self.root_directory.try_clone()?;
        let mut current_relative = PathBuf::new();
        let mut ignore_rules = Vec::new();
        if let Some(ignore) = gitignore_from_directory(&start, &self.root)? {
            ignore_rules.push(ignore);
        }
        for component in start_relative.components() {
            let descriptor = openat(
                &start,
                component.as_os_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|error| secure_path_error(relative, error.into()))?;
            start = descriptor.into();
            current_relative.push(component.as_os_str());
            if let Some(ignore) =
                gitignore_from_directory(&start, &self.root.join(&current_relative))?
            {
                ignore_rules.push(ignore);
            }
        }
        let ignore_refs = ignore_rules.iter().collect::<Vec<_>>();
        let mut listing = FileListing {
            entries: Vec::new(),
            truncated: false,
        };
        secure_walk_directory(
            start,
            &start_relative,
            0,
            &SecureWalkConfig {
                max_depth: depth,
                limit,
                workspace_root: &self.root,
            },
            &ignore_refs,
            &mut listing,
        )?;
        listing
            .entries
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(listing)
    }

    #[cfg(unix)]
    fn replace_workspace_file(
        &self,
        relative: &str,
        expected_sha256: Option<&str>,
        content: &[u8],
    ) -> Result<Option<String>, ToolError> {
        use rustix::fs::{
            AtFlags, Mode, OFlags, RenameFlags, openat, renameat, renameat_with, unlinkat,
        };

        let (parent, leaf) = self.secure_parent(relative)?;
        let current = match openat(
            &parent,
            &leaf,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(descriptor) => Some(fs::File::from(descriptor)),
            Err(error) => {
                let error: std::io::Error = error.into();
                if error.kind() == std::io::ErrorKind::NotFound {
                    None
                } else {
                    return Err(secure_path_error(relative, error));
                }
            }
        };
        let (previous_sha256, permissions) = if let Some(file) = current {
            let permissions = file.metadata()?.permissions();
            let hash = sha256(&read_bounded_regular_handle(file)?);
            let expected = expected_sha256.ok_or(ToolError::MissingExpectedHash)?;
            if expected != hash {
                return Err(ToolError::ConcurrentModification);
            }
            (Some(hash), Some(permissions))
        } else {
            if expected_sha256.is_some() {
                return Err(ToolError::ConcurrentModification);
            }
            (None, None)
        };

        let temp_name = OsString::from(format!(
            ".opencoding-write-{}-{}",
            std::process::id(),
            TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let descriptor = openat(
            &parent,
            &temp_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| ToolError::Io(error.into()))?;
        let mut temp = fs::File::from(descriptor);
        let result = (|| -> Result<(), ToolError> {
            temp.write_all(content)?;
            temp.sync_all()?;
            if let Some(permissions) = permissions {
                temp.set_permissions(permissions)?;
                temp.sync_all()?;
            }
            drop(temp);
            if previous_sha256.is_some() {
                renameat(&parent, &temp_name, &parent, &leaf)
                    .map_err(|error| ToolError::Io(error.into()))?;
            } else {
                renameat_with(&parent, &temp_name, &parent, &leaf, RenameFlags::NOREPLACE)
                    .map_err(|error| {
                        let error: std::io::Error = error.into();
                        if error.kind() == std::io::ErrorKind::AlreadyExists {
                            ToolError::ConcurrentModification
                        } else {
                            ToolError::Io(error)
                        }
                    })?;
            }
            parent.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = unlinkat(&parent, &temp_name, AtFlags::empty());
        }
        result?;
        Ok(previous_sha256)
    }

    #[cfg(not(unix))]
    fn replace_workspace_file(
        &self,
        relative: &str,
        expected_sha256: Option<&str>,
        content: &[u8],
    ) -> Result<Option<String>, ToolError> {
        let path = self.resolve(relative, true)?;
        let (previous_sha256, permissions) = if path.exists() {
            let current = read_bounded_regular_file(&path)?;
            let hash = sha256(&current);
            let expected = expected_sha256.ok_or(ToolError::MissingExpectedHash)?;
            if expected != hash {
                return Err(ToolError::ConcurrentModification);
            }
            (Some(hash), Some(fs::metadata(&path)?.permissions()))
        } else {
            if expected_sha256.is_some() {
                return Err(ToolError::ConcurrentModification);
            }
            (None, None)
        };
        let parent = path
            .parent()
            .ok_or_else(|| ToolError::Boundary(relative.into()))?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(content)?;
        temp.as_file().sync_all()?;
        if let Some(permissions) = permissions {
            temp.as_file().set_permissions(permissions)?;
        }
        temp.persist(&path)
            .map_err(|error| ToolError::Io(error.error))?;
        Ok(previous_sha256)
    }

    #[cfg(unix)]
    fn remove_workspace_file(
        &self,
        relative: &str,
        expected_sha256: &str,
    ) -> Result<(), ToolError> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};

        let (parent, leaf) = self.secure_parent(relative)?;
        let current = openat(
            &parent,
            &leaf,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map(fs::File::from)
        .map_err(|error| secure_path_error(relative, error.into()))?;
        if sha256(&read_bounded_regular_handle(current)?) != expected_sha256 {
            return Err(ToolError::ConcurrentModification);
        }
        unlinkat(&parent, &leaf, AtFlags::empty())
            .map_err(|error| secure_path_error(relative, error.into()))?;
        parent.sync_all()?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn remove_workspace_file(
        &self,
        relative: &str,
        expected_sha256: &str,
    ) -> Result<(), ToolError> {
        self.verify_file_hash(relative, expected_sha256)?;
        fs::remove_file(self.resolve(relative, false)?)?;
        Ok(())
    }

    fn reject_sensitive(&self, path: &Path) -> Result<(), ToolError> {
        if sensitive_path(path)
            || path
                .components()
                .filter_map(|component| component.as_os_str().to_str())
                .any(|component| component.eq_ignore_ascii_case(".git"))
        {
            return Err(ToolError::Sensitive(path.display().to_string()));
        }
        Ok(())
    }

    fn sensitive_workspace_uris(&self) -> Result<Vec<String>, ToolError> {
        sensitive_workspace_uris(&self.root)
    }
}

pub fn sensitive_workspace_uris(root: &Path) -> Result<Vec<String>, ToolError> {
    const MAX_SENSITIVE_PATHS: usize = 512;
    let root = root.canonicalize()?;
    let mut denied = Vec::new();
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(false)
        .build();
    for item in walker.skip(1) {
        let entry = item.map_err(|error| ToolError::Io(std::io::Error::other(error)))?;
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| ToolError::Boundary(entry.path().display().to_string()))?;
        let sensitive_git_config = relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            == [".git", "config"]
            && git_config_contains_credentials(entry.path());
        if sensitive_path(relative) || sensitive_git_config {
            if denied.len() == MAX_SENSITIVE_PATHS {
                return Err(ToolError::Sensitive(
                    "workspace contains too many sensitive paths".into(),
                ));
            }
            let uri = if entry.path().is_dir() {
                url::Url::from_directory_path(entry.path())
            } else {
                url::Url::from_file_path(entry.path())
            }
            .map_err(|_| ToolError::Boundary(entry.path().display().to_string()))?;
            denied.push(uri.to_string());
        }
    }
    Ok(denied)
}

fn git_config_contains_credentials(path: &Path) -> bool {
    let Ok(bytes) = read_bounded_regular_file(path) else {
        // A malformed, oversized or concurrently replaced Git config is not a
        // safe input to expose to an untrusted command.
        return true;
    };
    let lower = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    if lower.lines().any(|line| {
        let compact = line.trim();
        compact.contains("extraheader")
            || compact.contains("authorization")
            || compact.starts_with("helper")
            || compact.contains("password")
            || compact.contains("access_token")
            || compact.contains("private_token")
            || compact.contains("oauth_token")
    }) {
        return true;
    }
    lower.split_whitespace().any(|word| {
        let Some((_, authority_and_path)) = word.split_once("://") else {
            return false;
        };
        let authority = authority_and_path.split('/').next().unwrap_or_default();
        authority
            .split_once('@')
            .is_some_and(|(userinfo, _)| userinfo.contains(':'))
    })
}

fn sensitive_name(lower: &str) -> bool {
    let environment_template = lower.starts_with(".env.")
        && [".example", ".sample", ".template"]
            .iter()
            .any(|suffix| lower.ends_with(suffix));
    SENSITIVE_COMPONENTS.contains(&lower)
        || SENSITIVE_FILES.contains(&lower)
        || lower.starts_with(".env.") && !environment_template
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.ends_with(".kdbx")
}

fn sensitive_path(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if components.iter().any(|component| sensitive_name(component)) {
        return true;
    }
    components.windows(2).any(|parts| {
        parts[0] == ".config"
            && matches!(
                parts[1].as_str(),
                "gcloud" | "gh" | "hub" | "op" | "1password"
            )
    }) || components
        .windows(3)
        .any(|parts| parts == [".local", "share", "keyrings"])
}

fn read_bounded_regular_file(path: &Path) -> Result<Vec<u8>, ToolError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::Invalid("path is not a regular file".into()));
    }
    if metadata.len() > MAX_TOOL_FILE_BYTES {
        return Err(ToolError::Invalid(
            "file exceeds the 16 MiB file-tool limit".into(),
        ));
    }
    let file = fs::File::open(path)?;
    let opened = file.metadata()?;
    if !opened.file_type().is_file() || opened.len() != metadata.len() {
        return Err(ToolError::ConcurrentModification);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(ToolError::ConcurrentModification);
        }
    }
    let mut content = Vec::with_capacity(opened.len() as usize);
    file.take(MAX_TOOL_FILE_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_TOOL_FILE_BYTES {
        return Err(ToolError::Invalid(
            "file grew beyond the 16 MiB file-tool limit".into(),
        ));
    }
    Ok(content)
}

#[cfg(unix)]
fn read_bounded_regular_handle(file: fs::File) -> Result<Vec<u8>, ToolError> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(ToolError::Invalid("path is not a regular file".into()));
    }
    if metadata.len() > MAX_TOOL_FILE_BYTES {
        return Err(ToolError::Invalid(
            "file exceeds the 16 MiB file-tool limit".into(),
        ));
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_TOOL_FILE_BYTES + 1)
        .read_to_end(&mut content)?;
    if content.len() as u64 > MAX_TOOL_FILE_BYTES {
        return Err(ToolError::Invalid(
            "file grew beyond the 16 MiB file-tool limit".into(),
        ));
    }
    Ok(content)
}

#[cfg(unix)]
fn open_directory_no_follow(path: &Path) -> Result<fs::File, ToolError> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| ToolError::Io(error.into()))?;
    Ok(descriptor.into())
}

#[cfg(unix)]
fn secure_path_error(relative: &str, error: std::io::Error) -> ToolError {
    if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
        ToolError::Boundary(relative.into())
    } else {
        ToolError::Io(error)
    }
}

#[cfg(unix)]
fn gitignore_from_directory(
    directory: &fs::File,
    logical_directory: &Path,
) -> Result<Option<ignore::gitignore::Gitignore>, ToolError> {
    use ignore::gitignore::GitignoreBuilder;
    use rustix::fs::{Mode, OFlags, openat};

    let descriptor = match openat(
        directory,
        ".gitignore",
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            let error: std::io::Error = error.into();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(secure_path_error(
                &logical_directory.join(".gitignore").display().to_string(),
                error,
            ));
        }
    };
    let bytes = read_bounded_regular_handle(fs::File::from(descriptor))?;
    let mut builder = GitignoreBuilder::new(logical_directory);
    for line in String::from_utf8_lossy(&bytes).lines() {
        builder
            .add_line(Some(logical_directory.join(".gitignore")), line)
            .map_err(|error| ToolError::Invalid(format!("invalid .gitignore: {error}")))?;
    }
    Ok(Some(builder.build().map_err(|error| {
        ToolError::Invalid(format!("invalid .gitignore: {error}"))
    })?))
}

#[cfg(unix)]
fn ignored_by_gitignore_stack(
    ignore_rules: &[&ignore::gitignore::Gitignore],
    absolute_path: &Path,
    is_directory: bool,
) -> bool {
    let mut ignored = false;
    for rules in ignore_rules {
        match rules.matched_path_or_any_parents(absolute_path, is_directory) {
            ignore::Match::Ignore(_) => ignored = true,
            ignore::Match::Whitelist(_) => ignored = false,
            ignore::Match::None => {}
        }
    }
    ignored
}

#[cfg(unix)]
struct SecureWalkConfig<'a> {
    max_depth: usize,
    limit: usize,
    workspace_root: &'a Path,
}

#[cfg(unix)]
fn secure_walk_directory(
    directory: fs::File,
    parent_relative: &Path,
    current_depth: usize,
    config: &SecureWalkConfig<'_>,
    ignore_rules: &[&ignore::gitignore::Gitignore],
    listing: &mut FileListing,
) -> Result<(), ToolError> {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, openat, statat};
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let entries = Dir::read_from(&directory).map_err(|error| ToolError::Io(error.into()))?;
    for entry in entries {
        let entry = entry.map_err(|error| ToolError::Io(error.into()))?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        let relative = parent_relative.join(name);
        if relative
            .components()
            .any(|component| component.as_os_str() == ".git")
            || sensitive_path(&relative)
        {
            continue;
        }
        let metadata = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => metadata,
            Err(error) if std::io::Error::from(error).kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(ToolError::Io(error.into())),
        };
        let file_type = FileType::from_raw_mode(metadata.st_mode);
        let is_directory = file_type == FileType::Directory;
        if ignored_by_gitignore_stack(
            ignore_rules,
            &config.workspace_root.join(&relative),
            is_directory,
        ) {
            continue;
        }
        if listing.entries.len() == config.limit {
            listing.truncated = true;
            return Ok(());
        }
        listing.entries.push(FileEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            kind: match file_type {
                FileType::Directory => "directory",
                FileType::RegularFile => "file",
                _ => "other",
            }
            .into(),
            bytes: (file_type == FileType::RegularFile)
                .then(|| u64::try_from(metadata.st_size).ok())
                .flatten(),
        });
        if is_directory && current_depth < config.max_depth {
            match openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            ) {
                Ok(child) => {
                    let child = fs::File::from(child);
                    let child_ignore =
                        gitignore_from_directory(&child, &config.workspace_root.join(&relative))?;
                    let mut child_rules = ignore_rules.to_vec();
                    if let Some(ref rules) = child_ignore {
                        child_rules.push(rules);
                    }
                    secure_walk_directory(
                        child,
                        &relative,
                        current_depth + 1,
                        config,
                        &child_rules,
                        listing,
                    )?;
                }
                Err(error) => {
                    let error: std::io::Error = error.into();
                    if !matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ) && error.raw_os_error() != Some(rustix::io::Errno::LOOP.raw_os_error())
                    {
                        return Err(ToolError::Io(error));
                    }
                }
            }
        }
        if listing.truncated {
            return Ok(());
        }
    }
    Ok(())
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

struct RuntimeSupport {
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
    environment: BTreeMap<String, String>,
}

fn build_runtime_support(
    executable: &Path,
    scratch: &Path,
    workspace: &Path,
    dependency_cache_base: &Path,
    runtime_home: Option<&Path>,
) -> Result<RuntimeSupport, ToolError> {
    let dependency_cache = workspace_dependency_cache_root(dependency_cache_base, workspace)?;
    build_runtime_support_with_cache(executable, scratch, &dependency_cache, runtime_home)
}

fn build_runtime_support_with_cache(
    executable: &Path,
    scratch: &Path,
    dependency_cache: &Path,
    runtime_home: Option<&Path>,
) -> Result<RuntimeSupport, ToolError> {
    let mut roots = Vec::new();
    let mut writable_roots = Vec::new();
    let mut environment = BTreeMap::new();
    let sandbox_home = scratch.join("home");
    let sandbox_cache = scratch.join("cache");
    fs::create_dir_all(&sandbox_home)?;
    fs::create_dir_all(&sandbox_cache)?;
    environment.insert("HOME".into(), sandbox_home.to_string_lossy().into_owned());
    environment.insert(
        "XDG_CACHE_HOME".into(),
        sandbox_cache.to_string_lossy().into_owned(),
    );
    let name = executable
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if let Some(package) = managed_package_runtime_root(executable, runtime_home) {
        roots.push(package);
    }
    if let Some(shim) = managed_home_shim(executable, runtime_home) {
        roots.push(shim.root.clone());
        for (name, value) in shim.environment {
            environment.insert(name.into(), value);
        }
        let mut paths = vec![shim.shims, shim.root.join("bin")];
        paths.extend(
            ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
                .into_iter()
                .map(PathBuf::from),
        );
        environment.insert(
            "PATH".into(),
            std::env::join_paths(paths)
                .map_err(|_| ToolError::Invalid("managed runtime PATH is invalid".into()))?
                .to_string_lossy()
                .into_owned(),
        );
    }
    if matches!(name, "node" | "npm" | "npx" | "corepack") {
        let npm_cache = private_cache_subdirectory(dependency_cache, "npm")?;
        writable_roots.push(npm_cache.clone());
        environment.insert("NPM_CONFIG_USERCONFIG".into(), "/dev/null".into());
        environment.insert(
            "NPM_CONFIG_CACHE".into(),
            npm_cache.to_string_lossy().into_owned(),
        );
    }
    let rust_tool = matches!(
        name,
        "cargo" | "rustc" | "rustdoc" | "rustfmt" | "clippy-driver" | "rustup"
    );
    if rust_tool {
        let real_home = std::env::var_os("HOME").map(PathBuf::from);
        let rustup_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| real_home.as_ref().map(|home| home.join(".rustup")));
        if let Some(rustup_home) = rustup_home.filter(|path| path.is_dir()) {
            let rustup_home = rustup_home.canonicalize()?;
            roots.push(rustup_home.clone());
            environment.insert(
                "RUSTUP_HOME".into(),
                rustup_home.to_string_lossy().into_owned(),
            );
        }
        let isolated_cargo = scratch.join("cargo-home");
        fs::create_dir_all(&isolated_cargo)?;
        #[cfg(unix)]
        for directory in ["registry", "git"] {
            use std::os::unix::fs::symlink;

            let persistent =
                private_cache_subdirectory(dependency_cache, &format!("cargo-{directory}"))?;
            symlink(&persistent, isolated_cargo.join(directory))?;
            writable_roots.push(persistent);
        }
        environment.insert(
            "CARGO_HOME".into(),
            isolated_cargo.to_string_lossy().into_owned(),
        );
    }

    if matches!(name, "go" | "gofmt") {
        let go_build_cache = private_cache_subdirectory(dependency_cache, "go-build")?;
        let go_module_cache = private_cache_subdirectory(dependency_cache, "go-mod")?;
        writable_roots.extend([go_build_cache.clone(), go_module_cache.clone()]);
        environment.insert(
            "GOCACHE".into(),
            go_build_cache.to_string_lossy().into_owned(),
        );
        environment.insert(
            "GOMODCACHE".into(),
            go_module_cache.to_string_lossy().into_owned(),
        );
        environment.insert(
            "GOPATH".into(),
            scratch.join("go").to_string_lossy().into_owned(),
        );
    }

    if matches!(name, "java" | "javac" | "gradle" | "gradlew")
        && let Some(java_home) = std::env::var_os("JAVA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir())
            .and_then(|path| path.canonicalize().ok())
            .filter(|path| {
                !std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .is_some_and(|home| path.starts_with(home))
            })
    {
        roots.push(java_home.clone());
        environment.insert("JAVA_HOME".into(), java_home.to_string_lossy().into_owned());
    }

    roots.sort();
    roots.dedup();
    writable_roots.sort();
    writable_roots.dedup();
    Ok(RuntimeSupport {
        readable_roots: roots,
        writable_roots,
        environment,
    })
}

fn tool_dependency_cache_base() -> Result<PathBuf, ToolError> {
    tool_dependency_cache_base_from(|name| std::env::var_os(name), cfg!(windows))
}

fn environment_home_directory(
    value: impl Fn(&str) -> Option<std::ffi::OsString>,
    windows: bool,
) -> Option<PathBuf> {
    value("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            windows
                .then(|| value("USERPROFILE"))
                .flatten()
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        })
}

fn tool_dependency_cache_base_from(
    value: impl Fn(&str) -> Option<std::ffi::OsString>,
    windows: bool,
) -> Result<PathBuf, ToolError> {
    if let Some(configured) = value("OPENCODING_TOOL_CACHE_DIR") {
        let configured = PathBuf::from(configured);
        if !configured.is_absolute() {
            return Err(ToolError::Invalid(
                "OPENCODING_TOOL_CACHE_DIR must be absolute".into(),
            ));
        }
        return Ok(configured);
    }
    if let Some(base) = value("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(base.join("opencoding").join("tool-dependencies"));
    }
    if let Some(home) = value("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(home
            .join(".cache")
            .join("opencoding")
            .join("tool-dependencies"));
    }
    if windows {
        if let Some(local_app_data) = value("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            return Ok(local_app_data
                .join("Opencoding")
                .join("Cache")
                .join("tool-dependencies"));
        }
        if let Some(profile) = value("USERPROFILE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            return Ok(profile
                .join("AppData")
                .join("Local")
                .join("Opencoding")
                .join("Cache")
                .join("tool-dependencies"));
        }
    }
    Err(ToolError::Invalid(
        "XDG_CACHE_HOME, HOME, Windows LOCALAPPDATA/USERPROFILE, or OPENCODING_TOOL_CACHE_DIR is required"
            .into(),
    ))
}

fn workspace_dependency_cache_root(base: &Path, workspace: &Path) -> Result<PathBuf, ToolError> {
    let workspace = workspace.canonicalize()?;
    if !workspace.is_dir() {
        return Err(ToolError::Invalid(
            "dependency cache workspace must be a directory".into(),
        ));
    }
    let digest = sha256(workspace.to_string_lossy().as_bytes());
    private_cache_subdirectory(base, &format!("workspace-{}", &digest[..32]))
}

fn private_cache_subdirectory(root: &Path, name: &str) -> Result<PathBuf, ToolError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ToolError::Invalid("invalid dependency cache name".into()));
    }
    let root = private_cache_directory(root)?;
    private_cache_directory(&root.join(name))
}

fn private_cache_directory(path: &Path) -> Result<PathBuf, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::Invalid(
            "dependency cache path must be absolute".into(),
        ));
    }
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ToolError::Invalid(
                "dependency cache must be a real directory".into(),
            ));
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(path.canonicalize()?)
}

#[cfg(all(test, unix))]
fn find_program(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    find_program_in(
        program,
        std::env::split_paths(&paths).filter(|path| path.is_absolute()),
        &platform_executable_extensions(),
    )
}

fn stage_executable(source: &Path, scratch: &Path, role: &str) -> Result<PathBuf, ToolError> {
    let file_name = source
        .file_name()
        .ok_or_else(|| ToolError::Invalid("resolved program has no file name".into()))?
        .to_owned();
    let source = source.canonicalize()?;
    if !source.is_file() {
        return Err(ToolError::Invalid(
            "resolved program is not a regular file".into(),
        ));
    }
    let directory = scratch.join("runtime").join(role);
    fs::create_dir_all(&directory)?;
    let destination = directory.join(file_name);
    fs::copy(&source, &destination)?;
    Ok(destination)
}

fn stage_rustup_companions(source: &Path, staged: &Path) -> Result<(), ToolError> {
    if !matches!(
        source.file_name().and_then(|name| name.to_str()),
        Some("rustup" | "rustup-init")
    ) || staged.file_name().and_then(|name| name.to_str()) != Some("cargo")
    {
        return Ok(());
    }
    let parent = staged
        .parent()
        .ok_or_else(|| ToolError::Invalid("staged cargo has no parent".into()))?;
    for name in ["rustc", "rustdoc"] {
        fs::hard_link(staged, parent.join(name))
            .or_else(|_| fs::copy(staged, parent.join(name)).map(|_| ()))?;
    }
    Ok(())
}

fn managed_package_runtime_root(canonical: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let canonical = canonical.canonicalize().ok()?;
    if let Some(home) = home.and_then(|path| path.canonicalize().ok())
        && canonical.starts_with(&home)
    {
        return managed_home_runtime_root(&canonical, &home);
    }
    for cellar in ["/opt/homebrew/Cellar", "/usr/local/Cellar"] {
        let cellar = Path::new(cellar);
        if let Ok(relative) = canonical.strip_prefix(cellar) {
            let mut components = relative.components();
            let package = components.next()?.as_os_str();
            let version = components.next()?.as_os_str();
            return Some(cellar.join(package).join(version));
        }
    }
    if let Some(package) = node_package_runtime_root(&canonical) {
        return Some(package);
    }
    None
}

fn node_package_runtime_root(canonical: &Path) -> Option<PathBuf> {
    if ![
        "/opt/hostedtoolcache/node",
        "/opt/homebrew/lib/node_modules",
        "/usr/local/lib/node_modules",
    ]
    .into_iter()
    .any(|root| canonical.starts_with(root))
    {
        return None;
    }
    let components = canonical.components().collect::<Vec<_>>();
    let mut package_root = None;
    for index in 0..components.len().saturating_sub(2) {
        if components[index].as_os_str() != std::ffi::OsStr::new("lib")
            || components[index + 1].as_os_str() != std::ffi::OsStr::new("node_modules")
        {
            continue;
        }
        let package = components.get(index + 2)?.as_os_str();
        let package_end = if package.to_string_lossy().starts_with('@') {
            components.get(index + 3)?;
            index + 3
        } else {
            index + 2
        };
        package_root = Some(components[..=package_end].iter().collect());
    }
    package_root
}

fn managed_home_runtime_root(canonical: &Path, home: &Path) -> Option<PathBuf> {
    let home = home.canonicalize().ok()?;
    let relative = canonical.strip_prefix(&home).ok()?;
    let components = relative.components().collect::<Vec<_>>();
    let value = |index: usize| components.get(index).map(|part| part.as_os_str());
    let depth = if value(0) == Some(std::ffi::OsStr::new(".nvm"))
        && value(1) == Some(std::ffi::OsStr::new("versions"))
        && value(2) == Some(std::ffi::OsStr::new("node"))
        && value(3).is_some()
    {
        4
    } else if value(0) == Some(std::ffi::OsStr::new(".pyenv"))
        && value(1) == Some(std::ffi::OsStr::new("versions"))
        && value(2).is_some()
    {
        3
    } else if value(0) == Some(std::ffi::OsStr::new(".asdf"))
        && value(1) == Some(std::ffi::OsStr::new("installs"))
        && value(2).is_some()
        && value(3).is_some()
    {
        4
    } else if value(0) == Some(std::ffi::OsStr::new(".local"))
        && value(1) == Some(std::ffi::OsStr::new("share"))
        && value(2) == Some(std::ffi::OsStr::new("mise"))
        && value(3) == Some(std::ffi::OsStr::new("installs"))
        && value(4).is_some()
        && value(5).is_some()
    {
        6
    } else if value(0) == Some(std::ffi::OsStr::new(".volta"))
        && value(1) == Some(std::ffi::OsStr::new("tools"))
        && value(2) == Some(std::ffi::OsStr::new("image"))
        && value(3).is_some()
        && value(4).is_some()
    {
        5
    } else if value(0) == Some(std::ffi::OsStr::new(".conda"))
        && value(1) == Some(std::ffi::OsStr::new("envs"))
        && value(2).is_some()
    {
        3
    } else if matches!(
        value(0).and_then(|part| part.to_str()),
        Some("anaconda3" | "miniconda3" | "miniforge3")
    ) {
        if value(1) == Some(std::ffi::OsStr::new("envs")) && value(2).is_some() {
            3
        } else {
            1
        }
    } else {
        return None;
    };
    let mut root = home;
    for component in &components[..depth] {
        root.push(component.as_os_str());
    }
    let root = root.canonicalize().ok()?;
    (root.is_dir() && canonical.starts_with(&root)).then_some(root)
}

struct ManagedHomeShim {
    root: PathBuf,
    shims: PathBuf,
    environment: Vec<(&'static str, String)>,
}

fn managed_home_shim(executable: &Path, home: Option<&Path>) -> Option<ManagedHomeShim> {
    let home = home?.canonicalize().ok()?;
    let parent = executable.parent()?.canonicalize().ok()?;
    let (root, variables) = if parent == home.join(".pyenv/shims") {
        (home.join(".pyenv"), vec!["PYENV_ROOT"])
    } else if parent == home.join(".asdf/shims") {
        (home.join(".asdf"), vec!["ASDF_DATA_DIR", "ASDF_DIR"])
    } else if parent == home.join(".local/share/mise/shims") {
        (home.join(".local/share/mise"), vec!["MISE_DATA_DIR"])
    } else if parent == home.join(".volta/bin") {
        (home.join(".volta"), vec!["VOLTA_HOME"])
    } else {
        return None;
    };
    let root = root.canonicalize().ok()?;
    if !root.is_dir() || !parent.starts_with(&root) {
        return None;
    }
    let value = root.to_string_lossy().into_owned();
    Some(ManagedHomeShim {
        root,
        shims: parent,
        environment: variables
            .into_iter()
            .map(|name| (name, value.clone()))
            .collect(),
    })
}

fn shebang_interpreter(executable: &Path, search_path: &[PathBuf]) -> Option<PathBuf> {
    let canonical = executable.canonicalize().ok()?;
    let mut content = Vec::new();
    fs::File::open(canonical)
        .ok()?
        .take(8192)
        .read_to_end(&mut content)
        .ok()?;
    let first_line = content.split(|byte| *byte == b'\n').next()?;
    let shebang = std::str::from_utf8(first_line).ok()?.strip_prefix("#!")?;
    let mut parts = shebang.split_whitespace();
    let interpreter = parts.next()?;
    if interpreter == "/usr/bin/env" {
        return parts.next().and_then(|program| {
            find_program_in(
                program,
                search_path.iter().cloned(),
                &platform_executable_extensions(),
            )
        });
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
        let (directory, runtime) = runtime();
        #[cfg(not(unix))]
        let _ = &directory;
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
        for path in [
            ".env.production",
            ".docker/config.json",
            ".config/gcloud/credentials.db",
            "deploy/private.pem",
            "secrets.p12",
        ] {
            assert!(
                matches!(
                    runtime.read_file(path, 1, 1, 100),
                    Err(ToolError::Sensitive(_))
                ),
                "sensitive path was accepted: {path}"
            );
        }
        runtime
            .apply_replacement(FileReplacement {
                path: ".env.example".into(),
                expected_sha256: None,
                content: "API_TOKEN=replace-me\n".into(),
            })
            .unwrap();
        assert_eq!(
            runtime
                .read_file(".env.example", 1, 10, 1024)
                .unwrap()
                .content,
            "API_TOKEN=replace-me"
        );
        runtime
            .apply_replacement(FileReplacement {
                path: ".env.test.example".into(),
                expected_sha256: None,
                content: "TEST_TOKEN=replace-me\n".into(),
            })
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::write(directory.path().join(".env"), "TOKEN=secret").unwrap();
            symlink(".env", directory.path().join("apparently-safe")).unwrap();
            assert!(matches!(
                runtime.read_file("apparently-safe", 1, 10, 1024),
                Err(ToolError::Boundary(_) | ToolError::Sensitive(_))
            ));
            assert!(matches!(
                runtime.apply_replacement(FileReplacement {
                    path: "apparently-safe".into(),
                    expected_sha256: None,
                    content: "overwritten".into(),
                }),
                Err(ToolError::Boundary(_) | ToolError::Sensitive(_))
            ));
            assert_eq!(
                fs::read_to_string(directory.path().join(".env")).unwrap(),
                "TOKEN=secret"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_file_tools_do_not_follow_a_swapped_parent_symlink() {
        use std::{
            os::unix::fs::symlink,
            sync::atomic::{AtomicBool, AtomicUsize},
            thread,
        };

        let (directory, runtime) = runtime();
        let outside = tempfile::tempdir().unwrap();
        let safe = directory.path().join("safe");
        let parked = directory.path().join("safe.parked");
        fs::create_dir(&safe).unwrap();
        fs::write(safe.join("data.txt"), "inside").unwrap();
        fs::write(outside.path().join("data.txt"), "outside-sentinel").unwrap();
        fs::write(outside.path().join("outside-only-secret.txt"), "private").unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let swaps = Arc::new(AtomicUsize::new(0));
        let swap_stop = stop.clone();
        let swap_count = swaps.clone();
        let outside_path = outside.path().to_path_buf();
        let swapper = thread::spawn(move || {
            while !swap_stop.load(Ordering::Relaxed) {
                if fs::rename(&safe, &parked).is_ok() {
                    if symlink(&outside_path, &safe).is_ok() {
                        swap_count.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                        let _ = fs::remove_file(&safe);
                    }
                    let _ = fs::rename(&parked, &safe);
                }
                thread::yield_now();
            }
        });

        for round in 0..500 {
            if let Ok(listing) = runtime.list_files("safe", 2, 100) {
                assert!(
                    listing
                        .entries
                        .iter()
                        .all(|entry| !entry.path.contains("outside-only-secret"))
                );
            }
            let Ok(before) = runtime.snapshot_file("safe/data.txt") else {
                thread::yield_now();
                continue;
            };
            let Some(expected) = before.sha256.clone() else {
                continue;
            };
            let Ok(written) = runtime.apply_replacement(FileReplacement {
                path: "safe/data.txt".into(),
                expected_sha256: Some(expected),
                content: format!("inside-{round}"),
            }) else {
                continue;
            };
            if runtime
                .restore_file("safe/data.txt", &written.sha256, before.content.as_deref())
                .is_ok()
            {
                thread::yield_now();
            }
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();

        assert!(swaps.load(Ordering::Relaxed) > 0);
        assert_eq!(
            fs::read_to_string(outside.path().join("data.txt")).unwrap(),
            "outside-sentinel"
        );
        let stable = runtime.snapshot_file("safe/data.txt").unwrap();
        let stable_write = runtime
            .apply_replacement(FileReplacement {
                path: "safe/data.txt".into(),
                expected_sha256: stable.sha256.clone(),
                content: "stable-check".into(),
            })
            .unwrap();
        runtime
            .restore_file(
                "safe/data.txt",
                &stable_write.sha256,
                stable.content.as_deref(),
            )
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn commands_do_not_follow_a_replaced_workspace_path() {
        use std::os::unix::fs::symlink;

        let parent = tempfile::tempdir().unwrap();
        let workspace = parent.path().join("workspace");
        let parked = parent.path().join("authorized-workspace");
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(&workspace).unwrap();
        fs::write(outside.path().join("outside-only-secret.txt"), "private\n").unwrap();
        let uri = url::Url::from_directory_path(&workspace)
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();

        fs::rename(&workspace, &parked).unwrap();
        symlink(outside.path(), &workspace).unwrap();
        let readonly = runtime
            .run_with_profile(
                "sh",
                vec!["-c".into(), "cat outside-only-secret.txt".into()],
                Duration::from_secs(2),
                false,
                1024,
                false,
            )
            .await;
        let writable = runtime
            .run_with_profile(
                "sh",
                vec!["-c".into(), "printf leaked > created-by-command".into()],
                Duration::from_secs(2),
                false,
                1024,
                true,
            )
            .await;
        assert!(matches!(readonly, Err(ToolError::Boundary(_))));
        assert!(matches!(writable, Err(ToolError::Boundary(_))));
        assert!(!outside.path().join("created-by-command").exists());

        fs::remove_file(&workspace).unwrap();
        fs::rename(&parked, &workspace).unwrap();
    }

    #[test]
    fn file_tools_reject_oversized_and_sparse_files_before_loading_them() {
        let (dir, runtime) = runtime();
        let path = dir.path().join("oversized.bin");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_TOOL_FILE_BYTES + 1).unwrap();
        assert!(matches!(
            runtime.read_file("oversized.bin", 1, 1, 100),
            Err(ToolError::Invalid(message)) if message.contains("16 MiB")
        ));
        assert!(matches!(
            runtime.snapshot_file("oversized.bin"),
            Err(ToolError::Invalid(message)) if message.contains("16 MiB")
        ));
        assert!(matches!(
            runtime.apply_replacement(FileReplacement {
                path: "oversized.bin".into(),
                expected_sha256: Some("untrusted".into()),
                content: "replacement".into(),
            }),
            Err(ToolError::Invalid(message)) if message.contains("16 MiB")
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
    fn read_file_truncates_unicode_on_a_character_boundary_and_clamps_responses() {
        let (dir, runtime) = runtime();
        fs::write(
            dir.path().join("unicode.txt"),
            format!("{}🙂tail", "a".repeat(127)),
        )
        .unwrap();
        let unicode = runtime.read_file("unicode.txt", 1, 1, 128).unwrap();
        assert_eq!(unicode.content, "a".repeat(127));
        assert!(unicode.truncated);

        fs::write(
            dir.path().join("large.txt"),
            "x".repeat(MAX_READ_RESPONSE_BYTES + 1024),
        )
        .unwrap();
        let clamped = runtime.read_file("large.txt", 1, 1, usize::MAX).unwrap();
        assert_eq!(clamped.content.len(), MAX_READ_RESPONSE_BYTES);
        assert!(clamped.truncated);
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
    fn listing_respects_nested_gitignore_rules_and_reinclusions() {
        let (dir, runtime) = runtime();
        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(
            dir.path().join("nested/.gitignore"),
            "!keep.log\nsecret.txt\n",
        )
        .unwrap();
        fs::write(dir.path().join("nested/keep.log"), "visible").unwrap();
        fs::write(dir.path().join("nested/drop.log"), "ignored").unwrap();
        fs::write(dir.path().join("nested/secret.txt"), "ignored").unwrap();
        fs::write(dir.path().join("nested/visible.txt"), "visible").unwrap();

        let paths = runtime
            .list_files(".", 4, 100)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        assert!(paths.iter().any(|path| path == "nested/keep.log"));
        assert!(paths.iter().any(|path| path == "nested/visible.txt"));
        assert!(!paths.iter().any(|path| path == "nested/drop.log"));
        assert!(!paths.iter().any(|path| path == "nested/secret.txt"));
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
    fn symlinked_executable_is_staged_without_its_siblings() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let runtime_bin = directory.path().join("private-tools");
        let public_bin = directory.path().join("bin");
        fs::create_dir_all(&runtime_bin).unwrap();
        fs::create_dir_all(&public_bin).unwrap();
        let runtime = runtime_bin.join("runtime");
        fs::write(&runtime, "fixture").unwrap();
        fs::write(runtime_bin.join("sibling-secret"), "must stay private").unwrap();
        let public = public_bin.join("cargo");
        symlink(&runtime, &public).unwrap();

        let scratch = tempfile::tempdir().unwrap();
        let staged = stage_executable(&public, scratch.path(), "program").unwrap();
        assert_eq!(staged.file_name().unwrap(), "cargo");
        assert_eq!(fs::read_to_string(&staged).unwrap(), "fixture");
        assert!(!staged.parent().unwrap().join("sibling-secret").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rustup_init_multicall_proxy_keeps_requested_program_name() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let runtime_bin = tempfile::tempdir().unwrap();
        let public_bin = tempfile::tempdir().unwrap();
        let rustup_init = runtime_bin.path().join("rustup-init");
        fs::write(&rustup_init, "fixture").unwrap();
        symlink(&rustup_init, public_bin.path().join("cargo")).unwrap();
        let uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let runtime = ToolRuntime::open(&uri, Arc::new(TestRuntime))
            .unwrap()
            .with_runtime_environment(
                workspace.path().canonicalize().unwrap(),
                vec![public_bin.path().canonicalize().unwrap()],
            );

        let output = runtime
            .run_with_profile(
                "cargo",
                vec!["check".into()],
                Duration::from_secs(1),
                false,
                1024,
                true,
            )
            .await
            .unwrap();

        assert!(output.stdout.contains("/cargo"), "{output:?}");
        assert!(!output.stdout.contains("rustup-init"), "{output:?}");
    }

    #[test]
    fn version_managed_home_runtimes_expose_only_their_runtime_root() {
        let home = tempfile::tempdir().unwrap();
        let fixtures = [
            (".nvm/versions/node/v22.1.0/bin/node", 4_usize),
            (".pyenv/versions/3.13.1/bin/python", 3_usize),
            (".asdf/installs/golang/1.24.0/bin/go", 4_usize),
            (".local/share/mise/installs/node/23.0.0/bin/node", 6_usize),
            (".volta/tools/image/node/22.1.0/bin/node", 5_usize),
            (".conda/envs/review/bin/python", 3_usize),
            ("miniforge3/envs/review/bin/python", 3_usize),
        ];
        for (relative, depth) in fixtures {
            let executable = home.path().join(relative);
            fs::create_dir_all(executable.parent().unwrap()).unwrap();
            fs::write(&executable, "runtime").unwrap();
            let expected = relative
                .split('/')
                .take(depth)
                .fold(home.path().to_path_buf(), |path, part| path.join(part))
                .canonicalize()
                .unwrap();
            assert_eq!(
                managed_home_runtime_root(&executable.canonicalize().unwrap(), home.path()),
                Some(expected)
            );
        }
        let unrelated = home.path().join("private-tools/node");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, "runtime").unwrap();
        assert_eq!(
            managed_home_runtime_root(&unrelated.canonicalize().unwrap(), home.path()),
            None
        );
    }

    #[test]
    fn hosted_node_tools_expose_only_their_package_root() {
        let toolcache = Path::new("/opt/hostedtoolcache/node/24.8.0/x64");
        let npm = toolcache.join("lib/node_modules/npm/bin/npm-cli.js");
        let scoped = toolcache.join("lib/node_modules/@vendor/tool/bin/tool.js");

        assert_eq!(
            node_package_runtime_root(&npm),
            Some(toolcache.join("lib/node_modules/npm"))
        );
        assert_eq!(
            node_package_runtime_root(&scoped),
            Some(toolcache.join("lib/node_modules/@vendor/tool"))
        );
        assert_eq!(
            node_package_runtime_root(&toolcache.join("node_modules/npm/bin/npm-cli.js")),
            None
        );
        assert_eq!(
            node_package_runtime_root(Path::new(
                "/tmp/untrusted/lib/node_modules/npm/bin/npm-cli.js"
            )),
            None
        );
    }

    #[test]
    fn windows_native_directories_support_home_and_private_tool_cache_resolution() {
        let profile = if cfg!(windows) {
            PathBuf::from(r"C:\profiles\user")
        } else {
            PathBuf::from("/profiles/user")
        };
        let local_app_data_path = profile.join("AppData").join("Local");
        let local_app_data = BTreeMap::from([
            (
                "LOCALAPPDATA",
                local_app_data_path.as_os_str().to_os_string(),
            ),
            ("USERPROFILE", profile.as_os_str().to_os_string()),
        ]);
        let value = |name: &str| local_app_data.get(name).cloned();

        assert_eq!(
            environment_home_directory(value, true),
            Some(profile.clone())
        );
        assert_eq!(
            tool_dependency_cache_base_from(value, true).unwrap(),
            local_app_data_path
                .join("Opencoding")
                .join("Cache")
                .join("tool-dependencies")
        );

        let profile_only =
            |name: &str| (name == "USERPROFILE").then(|| profile.as_os_str().to_os_string());
        assert_eq!(
            tool_dependency_cache_base_from(profile_only, true).unwrap(),
            profile
                .join("AppData")
                .join("Local")
                .join("Opencoding")
                .join("Cache")
                .join("tool-dependencies")
        );
        assert!(tool_dependency_cache_base_from(profile_only, false).is_err());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn pyenv_shim_runs_with_only_the_manager_root_exposed() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        if cfg!(target_os = "linux") && find_program("bwrap").is_none() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let pyenv = home.path().join(".pyenv");
        let shim = pyenv.join("shims/python");
        let manager = pyenv.join("bin/pyenv");
        let runtime_program = pyenv.join("versions/3.13.1/bin/python");
        for program in [&shim, &manager, &runtime_program] {
            fs::create_dir_all(program.parent().unwrap()).unwrap();
        }
        fs::write(
            &shim,
            "#!/bin/sh\nexec \"$PYENV_ROOT/bin/pyenv\" exec python \"$@\"\n",
        )
        .unwrap();
        fs::write(
            &manager,
            "#!/bin/sh\n[ \"$1\" = exec ] && [ \"$2\" = python ] || exit 2\nshift 2\nexec \"$PYENV_ROOT/versions/3.13.1/bin/python\" \"$@\"\n",
        )
        .unwrap();
        fs::write(
            &runtime_program,
            "#!/bin/sh\nsecret=$(cat \"$PYENV_ROOT/../private.txt\" 2>/dev/null || true)\n[ -z \"$secret\" ] || { printf 'leak\\n'; exit 9; }\nprintf 'pyenv-runtime:%s\\n' \"$1\"\n",
        )
        .unwrap();
        fs::write(home.path().join("private.txt"), "home-secret-canary").unwrap();
        for program in [&shim, &manager, &runtime_program] {
            fs::set_permissions(program, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let uri = url::Url::from_directory_path(workspace.path())
            .unwrap()
            .to_string();
        let runtime = ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime))
            .unwrap()
            .with_dependency_cache_base(cache.path().join("tool-dependencies"))
            .with_runtime_environment(
                home.path().canonicalize().unwrap(),
                vec![pyenv.join("shims"), PathBuf::from("/bin")],
            );
        let output = runtime
            .run_with_profile(
                "python",
                vec!["works".into()],
                Duration::from_secs(10),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert_eq!(output.stdout.trim(), "pyenv-runtime:works");
        assert!(!output.stdout.contains("home-secret-canary"));
    }

    #[test]
    fn build_tools_reuse_private_dependency_caches_without_host_configuration() {
        let first_scratch = tempfile::tempdir().unwrap();
        let second_scratch = tempfile::tempdir().unwrap();
        let cache_parent = tempfile::tempdir().unwrap();
        let cache = cache_parent.path().join("dependency-cache");
        let first = build_runtime_support_with_cache(
            Path::new("/usr/bin/cargo"),
            first_scratch.path(),
            &cache,
            None,
        )
        .unwrap();
        let second = build_runtime_support_with_cache(
            Path::new("/usr/bin/cargo"),
            second_scratch.path(),
            &cache,
            None,
        )
        .unwrap();
        let encoded_roots = first
            .readable_roots
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!encoded_roots.contains("/.cargo/registry"));
        assert!(!encoded_roots.contains("/.cargo/git"));
        assert!(
            first.environment["CARGO_HOME"]
                .starts_with(&first_scratch.path().to_string_lossy()[..])
        );
        assert!(
            second.environment["CARGO_HOME"]
                .starts_with(&second_scratch.path().to_string_lossy()[..])
        );
        assert_ne!(
            first.environment["CARGO_HOME"],
            second.environment["CARGO_HOME"]
        );
        assert_eq!(first.writable_roots, second.writable_roots);
        // Unix can safely link isolated CARGO_HOME directories to the shared,
        // private registry and Git caches. Windows keeps CARGO_HOME entirely in
        // scratch space because the runtime does not create those symlinks.
        let expected_dependency_cache_roots = if cfg!(unix) { 2 } else { 0 };
        assert_eq!(first.writable_roots.len(), expected_dependency_cache_roots);
        assert!(
            first
                .writable_roots
                .iter()
                .all(|path| path.starts_with(cache.canonicalize().unwrap()))
        );
        #[cfg(unix)]
        for path in &first.writable_roots {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }

        let go = build_runtime_support_with_cache(
            Path::new("/usr/bin/go"),
            first_scratch.path(),
            &cache,
            None,
        )
        .unwrap();
        let encoded_roots = go
            .readable_roots
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!encoded_roots.contains("/go/pkg/mod"));
        let canonical_cache = cache.canonicalize().unwrap();
        assert!(go.environment["GOMODCACHE"].starts_with(&canonical_cache.to_string_lossy()[..]));
        assert!(go.environment["GOCACHE"].starts_with(&canonical_cache.to_string_lossy()[..]));
        assert!(go.environment["GOPATH"].starts_with(&first_scratch.path().to_string_lossy()[..]));
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
        fs::write(directory.path().join(".env"), "API_TOKEN=private").unwrap();
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
        runtime.search_text("needle", None, 1024).await.unwrap();

        let specifications = specifications.lock().unwrap();
        let dependency_cache_roots = if cfg!(unix) { 2 } else { 0 };
        assert_eq!(
            specifications[0].writable_root_uris.len(),
            1 + dependency_cache_roots
        );
        assert_eq!(
            specifications[1].writable_root_uris.len(),
            2 + dependency_cache_roots
        );
        assert!(
            specifications[1]
                .writable_root_uris
                .contains(&workspace_root_uri)
        );
        assert!(!specifications[0].browser_compatible);
        assert!(!specifications[1].browser_compatible);
        assert!(specifications[2].browser_compatible);
        assert_eq!(specifications[3].writable_root_uris.len(), 1);
        for specification in specifications.iter() {
            assert!(specification.environment_handles.contains_key("TMPDIR"));
            assert!(
                specification
                    .denied_read_uris
                    .iter()
                    .any(|uri| uri.ends_with("/.env"))
            );
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn commands_cannot_read_sensitive_workspace_files() {
        let directory = tempfile::tempdir().unwrap();
        let uri = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();
        let initial = runtime
            .run_with_profile(
                "sh",
                vec!["-c".into(), "printf ready".into()],
                Duration::from_secs(5),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_eq!(initial.exit_code, Some(0), "{initial:?}");
        fs::write(
            directory.path().join(".env.production"),
            "API_TOKEN=workspace-secret-canary",
        )
        .unwrap();
        fs::create_dir(directory.path().join("app")).unwrap();
        fs::write(
            directory.path().join("app/.env"),
            "API_TOKEN=nested-workspace-secret-canary",
        )
        .unwrap();
        fs::create_dir(directory.path().join(".git")).unwrap();
        fs::write(
            directory.path().join(".git/config"),
            "[http]\n\textraHeader = Authorization: Bearer git-secret-canary\n",
        )
        .unwrap();
        let output = runtime
            .run_with_profile(
                "sh",
                vec!["-c".into(), "cat .env.production".into()],
                Duration::from_secs(5),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_ne!(output.exit_code, Some(0), "{output:?}");
        assert!(!output.stdout.contains("workspace-secret-canary"));
        let git_config = runtime
            .run_with_profile(
                "sh",
                vec!["-c".into(), "cat .git/config".into()],
                Duration::from_secs(5),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_ne!(git_config.exit_code, Some(0), "{git_config:?}");
        assert!(!git_config.stdout.contains("git-secret-canary"));
        let rename = runtime
            .run_with_profile(
                "sh",
                vec![
                    "-c".into(),
                    "mv .env.production safe.txt 2>/dev/null || ln .env.production safe.txt 2>/dev/null; cat safe.txt 2>/dev/null"
                        .into(),
                ],
                Duration::from_secs(5),
                false,
                4096,
                true,
            )
            .await
            .unwrap();
        assert!(!rename.stdout.contains("workspace-secret-canary"));
        assert!(directory.path().join(".env.production").is_file());
        let nested_rename = runtime
            .run_with_profile(
                "sh",
                vec![
                    "-c".into(),
                    "mv app app2 2>/dev/null; cat app2/.env 2>/dev/null".into(),
                ],
                Duration::from_secs(5),
                false,
                4096,
                true,
            )
            .await
            .unwrap();
        assert!(
            !nested_rename
                .stdout
                .contains("nested-workspace-secret-canary"),
            "{nested_rename:?}"
        );
        assert!(directory.path().join("app/.env").is_file());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn common_rust_and_node_builds_run_inside_the_sandbox() {
        if cfg!(target_os = "linux") && find_program("bwrap").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname='sandbox-smoke'\nversion='0.1.0'\nedition='2024'\n\n[workspace]\n",
        )
        .unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"scripts":{"test":"node -e \"console.log('node-smoke')\""}}"#,
        )
        .unwrap();
        let uri = url::Url::from_directory_path(directory.path())
            .unwrap()
            .to_string();
        let runtime =
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime)).unwrap();
        if find_program("cargo").is_some() {
            let output = runtime
                .run_with_profile(
                    "cargo",
                    vec!["check".into(), "--offline".into()],
                    Duration::from_secs(30),
                    false,
                    64 * 1024,
                    true,
                )
                .await
                .unwrap();
            assert_eq!(output.exit_code, Some(0), "{output:?}");
        }
        if find_program("node").is_some() {
            let output = runtime
                .run_with_profile(
                    "node",
                    vec!["-e".into(), "console.log('node-smoke')".into()],
                    Duration::from_secs(10),
                    false,
                    64 * 1024,
                    false,
                )
                .await
                .unwrap();
            assert_eq!(output.exit_code, Some(0), "{output:?}");
        }
        if find_program("npm").is_some() {
            let output = runtime
                .run_with_profile(
                    "npm",
                    vec!["test".into(), "--ignore-scripts=false".into()],
                    Duration::from_secs(20),
                    false,
                    64 * 1024,
                    false,
                )
                .await
                .unwrap();
            assert_eq!(output.exit_code, Some(0), "{output:?}");
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn dependency_caches_are_reused_only_inside_the_same_workspace() {
        if find_program("node").is_none()
            || (cfg!(target_os = "linux") && find_program("bwrap").is_none())
        {
            return;
        }
        let workspace_a = tempfile::tempdir().unwrap();
        let workspace_b = tempfile::tempdir().unwrap();
        let cache_parent = tempfile::tempdir().unwrap();
        let cache_base = cache_parent.path().join("tool-dependencies");
        let open = |workspace: &tempfile::TempDir| {
            let uri = url::Url::from_directory_path(workspace.path())
                .unwrap()
                .to_string();
            ToolRuntime::open(&uri, Arc::new(opencoding_platform_runtime::NativeRuntime))
                .unwrap()
                .with_dependency_cache_base(cache_base.clone())
        };
        let runtime_a = open(&workspace_a);
        let runtime_b = open(&workspace_b);
        let cache_a = workspace_dependency_cache_root(&cache_base, workspace_a.path()).unwrap();
        let cache_b = workspace_dependency_cache_root(&cache_base, workspace_b.path()).unwrap();
        assert_ne!(cache_a, cache_b);

        let write = runtime_a
            .run_with_profile(
                "node",
                vec![
                    "-e".into(),
                    "require('fs').writeFileSync(process.env.NPM_CONFIG_CACHE + '/canary', 'workspace-a')"
                        .into(),
                ],
                Duration::from_secs(10),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_eq!(write.exit_code, Some(0), "{write:?}");
        let probe = format!(
            "try {{ console.log(require('fs').readFileSync({:?}, 'utf8')) }} catch (_) {{ console.log('isolated') }}",
            cache_a.join("npm/canary").to_string_lossy()
        );
        let cross_workspace = runtime_b
            .run_with_profile(
                "node",
                vec!["-e".into(), probe],
                Duration::from_secs(10),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_eq!(cross_workspace.exit_code, Some(0), "{cross_workspace:?}");
        assert_eq!(cross_workspace.stdout.trim(), "isolated");

        let same_workspace = runtime_a
            .run_with_profile(
                "node",
                vec![
                    "-e".into(),
                    "console.log(require('fs').readFileSync(process.env.NPM_CONFIG_CACHE + '/canary', 'utf8'))"
                        .into(),
                ],
                Duration::from_secs(10),
                false,
                4096,
                false,
            )
            .await
            .unwrap();
        assert_eq!(same_workspace.exit_code, Some(0), "{same_workspace:?}");
        assert_eq!(same_workspace.stdout.trim(), "workspace-a");
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
