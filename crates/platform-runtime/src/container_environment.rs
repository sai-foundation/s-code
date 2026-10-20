//! The command environment of a Harbor task container.
//!
//! Full-mode host commands normally start from a cleared, minimal environment.
//! Harbor benchmark bundles are built with the `harbor-container-env` feature:
//! their adapter captures the environment an ordinary command in the task
//! container gets (`env -0`, as the agent user), the daemon loads it once at
//! startup, and host commands then run in it, so a score reflects S-Code rather
//! than a PATH or HOME the task image never had. Without the feature nothing
//! can load or install an environment and [`installed`] is always `None`.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};
#[cfg(feature = "harbor-container-env")]
use std::{collections::BTreeSet, path::Path, sync::OnceLock};

#[cfg(all(feature = "harbor-container-env", not(unix)))]
compile_error!("the harbor-container-env feature supports only Unix task containers");

#[cfg(feature = "harbor-container-env")]
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
#[cfg(feature = "harbor-container-env")]
pub const MAX_SNAPSHOT_RECORDS: usize = 4096;

/// Shell bookkeeping that describes the capture rather than a command.
#[cfg(feature = "harbor-container-env")]
const VOLATILE_NAMES: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

/// The variables Full-mode host commands get instead of the minimal
/// environment. `Debug` shows names only.
pub struct ContainerEnvironment {
    variables: BTreeMap<OsString, OsString>,
}

impl ContainerEnvironment {
    pub fn variables(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.variables
            .iter()
            .map(|(name, value)| (name.as_os_str(), value.as_os_str()))
    }

    /// The absolute entries of this environment's PATH in their original
    /// order, so a program is found on the PATH it then runs with.
    pub fn search_path(&self) -> Vec<PathBuf> {
        self.variables
            .get(OsStr::new("PATH"))
            .map(|paths| {
                std::env::split_paths(paths)
                    .filter(|path| path.is_absolute())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for ContainerEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerEnvironment")
            .field("names", &self.variables.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(feature = "harbor-container-env")]
static INSTALLED: OnceLock<ContainerEnvironment> = OnceLock::new();

/// The environment the daemon installed for host commands, if any.
pub fn installed() -> Option<&'static ContainerEnvironment> {
    #[cfg(feature = "harbor-container-env")]
    {
        INSTALLED.get()
    }
    #[cfg(not(feature = "harbor-container-env"))]
    {
        None
    }
}

/// Set the process-wide container environment. It can be set only once.
#[cfg(feature = "harbor-container-env")]
pub fn install(environment: ContainerEnvironment) -> Result<(), String> {
    INSTALLED
        .set(environment)
        .map_err(|_| "the container environment is already installed".into())
}

/// What the daemon records about a loaded environment: names and a digest,
/// never values.
#[cfg(feature = "harbor-container-env")]
#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub mode: &'static str,
    pub count: usize,
    pub names: Vec<String>,
    /// SHA-256 of the `NAME=VALUE` records in name order, joined by NUL.
    pub sha256: String,
}

/// Load an `env -0` snapshot, delete it, and write its summary (mode 0600).
/// The snapshot must be a regular file that only this user can read. S-Code's
/// own `S_CODE_*` variables, the agent-only names Harbor passed to the agent,
/// the shell's volatile fields and any variable holding one of
/// `secret_values` are dropped; later duplicates win. Any failure is an
/// error, so the daemon never starts commands in an environment it did not
/// fully load.
#[cfg(feature = "harbor-container-env")]
pub fn load(
    snapshot: &Path,
    agent_names: &BTreeSet<OsString>,
    secret_values: &[OsString],
    summary: &Path,
) -> Result<(ContainerEnvironment, Summary), String> {
    use rustix::fs::{Mode, OFlags};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    };

    // Open without following a link, then check what was opened.
    let file = rustix::fs::open(
        snapshot,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(|error| format!("cannot open {}: {error}", snapshot.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", snapshot.display()))?;
    let private = metadata.is_file()
        && metadata.uid() == rustix::process::geteuid().as_raw()
        && metadata.mode() & 0o077 == 0;
    let mut bytes = Vec::new();
    if private {
        file.take(MAX_SNAPSHOT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read {}: {error}", snapshot.display()))?;
    }
    // The snapshot can hold the agent's credentials, so remove it before any
    // other check can fail.
    std::fs::remove_file(snapshot)
        .map_err(|error| format!("cannot delete {}: {error}", snapshot.display()))?;
    if !private {
        return Err(format!(
            "{} must be a regular file that only its owner, this user, can access",
            snapshot.display()
        ));
    }
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(format!(
            "{} is larger than {MAX_SNAPSHOT_BYTES} bytes",
            snapshot.display()
        ));
    }
    let mut records = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    if records.last().is_some_and(|record| record.is_empty()) {
        records.pop();
    }
    if records.len() > MAX_SNAPSHOT_RECORDS {
        return Err(format!(
            "{} has more than {MAX_SNAPSHOT_RECORDS} records",
            snapshot.display()
        ));
    }
    let mut variables = BTreeMap::new();
    for record in records {
        let Some(separator) = record.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let name = OsStr::from_bytes(&record[..separator]);
        let value = OsStr::from_bytes(&record[separator + 1..]);
        if name.is_empty()
            || name.as_bytes().starts_with(b"S_CODE_")
            || VOLATILE_NAMES.iter().any(|volatile| name == *volatile)
            || agent_names.contains(name)
            || secret_values
                .iter()
                .any(|secret| !secret.is_empty() && secret.as_os_str() == value)
        {
            continue;
        }
        variables.insert(name.to_owned(), value.to_owned());
    }

    let mut digest = Sha256::new();
    for (index, (name, value)) in variables.iter().enumerate() {
        if index > 0 {
            digest.update([0]);
        }
        digest.update(name.as_bytes());
        digest.update(b"=");
        digest.update(value.as_bytes());
    }
    let recorded = Summary {
        mode: "container",
        count: variables.len(),
        names: variables
            .keys()
            .map(|name| name.to_string_lossy().into_owned())
            .collect(),
        sha256: digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    let json = serde_json::to_vec(&recorded).map_err(|error| error.to_string())?;
    // Replace a stale summary instead of writing through whatever is there.
    match std::fs::remove_file(summary) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("cannot replace {}: {error}", summary.display()));
        }
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(summary)
        .and_then(|mut file| file.write_all(&json))
        .map_err(|error| format!("cannot write {}: {error}", summary.display()))?;
    Ok((ContainerEnvironment { variables }, recorded))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a snapshot the way the runner does: a file only its owner can read.
    #[cfg(feature = "harbor-container-env")]
    fn write_snapshot(path: &std::path::Path, bytes: &[u8]) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut file| file.write_all(bytes))
            .unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn write_executable(path: &std::path::Path, script: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Run a Full-mode host command the way `ToolRuntime::run_host` does,
    /// with an explicit container environment instead of the installed one.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn run_host(
        workspace: &std::path::Path,
        program: impl AsRef<std::path::Path>,
        args: &[&str],
        container: Option<&ContainerEnvironment>,
    ) -> crate::ProcessOutput {
        crate::NativeRuntime::execute_native(
            crate::ProcessSpec {
                program: program.as_ref().to_string_lossy().into_owned(),
                args: args.iter().map(|arg| arg.to_string()).collect(),
                cwd_uri: url::Url::from_directory_path(workspace)
                    .unwrap()
                    .to_string(),
                environment_handles: Default::default(),
                timeout: std::time::Duration::from_secs(10),
                network_enabled: true,
                browser_compatible: false,
                readable_root_uris: Vec::new(),
                writable_root_uris: Vec::new(),
                denied_read_uris: Vec::new(),
                output_limit_bytes: 64 * 1024,
                pinned_cwd: Some(std::sync::Arc::new(std::fs::File::open(workspace).unwrap())),
            },
            true,
            container,
        )
        .await
        .unwrap()
    }

    #[cfg(not(feature = "harbor-container-env"))]
    #[test]
    fn normal_builds_never_have_a_container_environment() {
        assert!(installed().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn search_path_keeps_absolute_entries_in_their_order() {
        let environment = ContainerEnvironment {
            variables: BTreeMap::from([(
                OsString::from("PATH"),
                OsString::from("/opt/venv/bin:relative::/usr/local/bin:/usr/bin"),
            )]),
        };
        assert_eq!(
            environment.search_path(),
            ["/opt/venv/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from)
        );
        let without_path = ContainerEnvironment {
            variables: BTreeMap::new(),
        };
        assert!(without_path.search_path().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn host_commands_run_in_the_container_environment_and_nothing_else() {
        let workspace = tempfile::tempdir().unwrap();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        for directory in [first.path(), second.path()] {
            write_executable(&directory.join("tool"), "#!/bin/sh\n");
        }
        let node = first.path().join("node");
        write_executable(
            &node,
            "#!/bin/sh\nprintf 'TMPDIR=%s OPENSSL_CONF=%s\\n' \"${TMPDIR-unset}\" \"${OPENSSL_CONF-unset}\"\n",
        );
        let path = std::env::join_paths([
            first.path(),
            second.path(),
            std::path::Path::new("/usr/bin"),
            std::path::Path::new("/bin"),
        ])
        .unwrap();
        let environment = ContainerEnvironment {
            variables: BTreeMap::from([
                (OsString::from("PATH"), path.clone()),
                (OsString::from("HOME"), home.path().into()),
                (OsString::from("CONTAINER_MARKER"), "from the image".into()),
            ]),
        };

        let output = run_host(
            workspace.path(),
            "/bin/sh",
            &["-c", "command -v tool; printf '%s\\n' \"$HOME\""],
            Some(&environment),
        )
        .await;
        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert_eq!(
            output.stdout,
            format!(
                "{}\n{}\n",
                first.path().join("tool").display(),
                home.path().display()
            )
        );

        // A program receives the container's variables and its own PWD, and
        // nothing from S-Code: no PATH rewrite, TMPDIR or daemon variable.
        let printed = run_host(workspace.path(), "/usr/bin/env", &[], Some(&environment)).await;
        let mut received = printed.stdout.lines().collect::<Vec<_>>();
        received.sort_unstable();
        assert_eq!(
            received,
            [
                "CONTAINER_MARKER=from the image".to_string(),
                format!("HOME={}", home.path().display()),
                format!("PATH={}", path.to_str().unwrap()),
                format!("PWD={}", workspace.path().canonicalize().unwrap().display()),
            ]
        );

        // Only the minimal environment gives a program named node its own
        // OPENSSL_CONF and a per-command TMPDIR.
        let contained = run_host(workspace.path(), &node, &[], Some(&environment)).await;
        assert_eq!(contained.stdout, "TMPDIR=unset OPENSSL_CONF=unset\n");
        let minimal = run_host(workspace.path(), &node, &[], None).await;
        assert!(
            minimal.stdout.contains("OPENSSL_CONF=/dev/null") && !minimal.stdout.contains("unset"),
            "{minimal:?}"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[tokio::test]
    async fn git_global_configuration_persists_through_the_container_home() {
        let git = std::path::Path::new("/usr/bin/git");
        if !git.is_file() {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let environment = ContainerEnvironment {
            variables: BTreeMap::from([
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), home.path().into()),
            ]),
        };
        let set = run_host(
            workspace.path(),
            git,
            &["config", "--global", "user.name", "Fixture User"],
            Some(&environment),
        )
        .await;
        assert_eq!(set.exit_code, Some(0), "{set:?}");
        let get = run_host(
            workspace.path(),
            git,
            &["config", "--global", "user.name"],
            Some(&environment),
        )
        .await;
        assert_eq!(get.stdout, "Fixture User\n", "{get:?}");
        assert!(home.path().join(".gitconfig").is_file());
    }

    #[cfg(feature = "harbor-container-env")]
    #[test]
    fn load_keeps_container_variables_and_writes_a_summary_without_values() {
        use sha2::{Digest, Sha256};
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let snapshot = directory.path().join("command.env");
        let summary = directory.path().join("command-env.json");
        let records: &[&[u8]] = &[
            b"PATH=/opt/venv/bin:/usr/local/bin:/usr/bin:/bin",
            b"HOME=/home/agent",
            b"BINARY=\xff\xfe",
            b"EQUALS=a=b=c",
            b"MULTILINE=first\nsecond",
            b"DUPLICATE=earlier",
            b"DUPLICATE=later",
            b"S_CODE_HARBOR_CREDENTIAL=s-code-canary-value",
            b"S_CODE_TOKEN=s-code-token-value",
            b"AGENT_ONLY=agent-canary-value",
            b"COPIED_KEY=model-key-canary",
            b"PWD=/app",
            b"OLDPWD=/",
            b"SHLVL=1",
            b"_=/installed-agent/run",
            b"=empty-name",
            b"NO_SEPARATOR",
            b"",
        ];
        let mut bytes = records.join(&0);
        bytes.push(0);
        write_snapshot(&snapshot, &bytes);

        let (environment, recorded) = load(
            &snapshot,
            &BTreeSet::from([OsString::from("AGENT_ONLY")]),
            &[OsString::from("model-key-canary")],
            &summary,
        )
        .unwrap();

        assert!(!snapshot.exists());
        let kept: &[&[u8]] = &[
            b"BINARY=\xff\xfe",
            b"DUPLICATE=later",
            b"EQUALS=a=b=c",
            b"HOME=/home/agent",
            b"MULTILINE=first\nsecond",
            b"PATH=/opt/venv/bin:/usr/local/bin:/usr/bin:/bin",
        ];
        assert_eq!(
            environment
                .variables()
                .map(|(name, value)| [name.as_bytes(), b"=", value.as_bytes()].concat())
                .collect::<Vec<_>>(),
            kept
        );
        let sha256 = Sha256::digest(kept.join(&0))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(recorded.sha256, sha256);
        let written = std::fs::read(&summary).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&written).unwrap(),
            serde_json::json!({
                "mode": "container",
                "count": 6,
                "names": ["BINARY", "DUPLICATE", "EQUALS", "HOME", "MULTILINE", "PATH"],
                "sha256": sha256,
            })
        );
        for value in [
            b"/home/agent".as_slice(),
            b"a=b=c",
            b"later",
            b"/opt/venv/bin",
            b"canary-value",
            b"token-value",
            b"model-key",
        ] {
            assert!(!written.windows(value.len()).any(|window| window == value));
        }
        assert_eq!(
            std::fs::metadata(&summary).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(feature = "harbor-container-env")]
    #[test]
    fn load_refuses_links_and_files_others_can_access() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let summary = directory.path().join("command-env.json");
        let target = directory.path().join("target.env");
        write_snapshot(&target, b"PATH=/usr/bin\0");
        let link = directory.path().join("link.env");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(load(&link, &BTreeSet::new(), &[], &summary).is_err());
        assert!(
            target.exists(),
            "the link's target is never read or removed"
        );

        let shared = directory.path().join("shared.env");
        write_snapshot(&shared, b"PATH=/usr/bin\0");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o640)).unwrap();
        let error = load(&shared, &BTreeSet::new(), &[], &summary).unwrap_err();
        assert!(error.contains("only its owner"), "{error}");
        assert!(!shared.exists());
        assert!(!summary.exists());
    }

    #[cfg(feature = "harbor-container-env")]
    #[test]
    fn load_accepts_the_limits_and_rejects_anything_larger() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = directory.path().join("command.env");
        let summary = directory.path().join("command-env.json");
        let largest = [b"LARGE=".as_slice(), &vec![b'x'; MAX_SNAPSHOT_BYTES - 6]].concat();
        for (bytes, accepted) in [
            (largest.clone(), true),
            ([largest.as_slice(), b"x"].concat(), false),
            (b"A=1\0".repeat(MAX_SNAPSHOT_RECORDS), true),
            (b"A=1\0".repeat(MAX_SNAPSHOT_RECORDS + 1), false),
        ] {
            write_snapshot(&snapshot, &bytes);
            let loaded = load(&snapshot, &BTreeSet::new(), &[], &summary);
            assert_eq!(loaded.is_ok(), accepted, "{loaded:?}");
            assert!(!snapshot.exists());
        }
    }

    #[cfg(all(
        feature = "harbor-container-env",
        any(target_os = "macos", target_os = "linux")
    ))]
    #[tokio::test]
    async fn s_code_and_agent_variables_never_reach_commands() {
        let workspace = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let snapshot = directory.path().join("command.env");
        write_snapshot(
            &snapshot,
            b"PATH=/usr/bin:/bin\0CONTAINER_MARKER=kept\0S_CODE_HARBOR_CREDENTIAL=s-code-canary-5c1e\0AGENT_ONLY=agent-canary-9b2d\0",
        );
        let (environment, _) = load(
            &snapshot,
            &BTreeSet::from([OsString::from("AGENT_ONLY")]),
            &[],
            &directory.path().join("command-env.json"),
        )
        .unwrap();

        let output = run_host(workspace.path(), "/usr/bin/env", &[], Some(&environment)).await;
        assert_eq!(output.exit_code, Some(0), "{output:?}");
        assert!(
            output
                .stdout
                .lines()
                .any(|line| line == "CONTAINER_MARKER=kept")
        );
        for canary in [
            "S_CODE_",
            "s-code-canary-5c1e",
            "AGENT_ONLY",
            "agent-canary-9b2d",
        ] {
            assert!(!output.stdout.contains(canary), "{output:?}");
        }
    }
}
