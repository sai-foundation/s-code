//! Harbor bundles only: once the daemon installs a task container's command
//! environment, a Full-mode host command is found on that environment's PATH
//! and runs in exactly that environment. Integration tests run in their own
//! process, so installing the process-wide environment is safe here.
#![cfg(all(
    feature = "harbor-container-env",
    any(target_os = "macos", target_os = "linux")
))]

use s_code_platform_runtime::{NativeRuntime, container_environment};
use s_code_tool_runtime::ToolRuntime;
use std::{
    collections::BTreeSet, ffi::OsString, os::unix::fs::PermissionsExt, sync::Arc, time::Duration,
};

#[tokio::test]
async fn host_commands_use_the_installed_container_environment() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    let home = directory.path().join("home");
    let workspace = directory.path().join("workspace");
    for path in [&bin, &home, &workspace] {
        std::fs::create_dir(path).unwrap();
    }
    // `probe` exists only on the container PATH, so finding it proves the
    // lookup uses that PATH rather than the daemon's.
    std::os::unix::fs::symlink("/usr/bin/env", bin.join("probe")).unwrap();

    let path = format!("{}:/usr/bin:/bin", bin.display());
    let snapshot = directory.path().join("command.env");
    std::fs::write(
        &snapshot,
        format!(
            "PATH={path}\0HOME={}\0CONTAINER_MARKER=kept\0S_CODE_CANARY=dropped\0\
             AGENT_ONLY=dropped\0PWD=/capture\0",
            home.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (environment, _) = container_environment::load(
        &snapshot,
        &BTreeSet::from([OsString::from("AGENT_ONLY")]),
        &[],
        &directory.path().join("command-env.json"),
    )
    .unwrap();
    container_environment::install(environment).unwrap();

    let workspace = workspace.canonicalize().unwrap();
    let uri = url::Url::from_directory_path(&workspace).unwrap();
    let runtime = ToolRuntime::open(uri.as_str(), Arc::new(NativeRuntime)).unwrap();
    let output = runtime
        .run_host("probe", Vec::new(), Duration::from_secs(10), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(output.exit_code, Some(0), "{}", output.stderr);
    let mut variables = output.stdout.lines().collect::<Vec<_>>();
    variables.sort_unstable();
    assert_eq!(
        variables,
        [
            "CONTAINER_MARKER=kept".to_string(),
            format!("HOME={}", home.display()),
            format!("PATH={path}"),
            format!("PWD={}", workspace.display()),
        ]
    );
}
