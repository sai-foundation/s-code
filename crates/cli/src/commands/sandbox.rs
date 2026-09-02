use crate::{
    CliExitStatus,
    api::{Api, completed_tool_result},
};
use anyhow::{Result, anyhow};
use opencoding_protocol::{ApprovalScope, Id};
use serde_json::json;
use std::io::{self, Write};

pub(crate) async fn run_sandbox_command(
    api: &Api,
    session_id: &Id,
    program: &str,
    args: &[String],
    profile: &str,
    network_enabled: bool,
    timeout_seconds: u64,
) -> Result<()> {
    let mut outcome = api
        .submit_tool(
            session_id,
            "run_command",
            json!({
                "program": program,
                "args": args,
                "timeout_seconds": timeout_seconds.min(600),
                "network_enabled": network_enabled,
                "sandbox_profile": profile,
                "max_bytes": 1048576
            }),
        )
        .await?;
    if outcome["outcome"].as_str() == Some("awaiting_approval") {
        let approval_id = outcome["approval"]["id"]
            .as_str()
            .ok_or_else(|| anyhow!("daemon omitted the sandbox approval identifier"))?;
        outcome = api.approval(approval_id, true, ApprovalScope::Once).await?;
    }
    match outcome["outcome"].as_str() {
        Some("completed") => {
            let result = completed_tool_result(outcome)?;
            if let Some(stdout) = result["stdout"].as_str() {
                print!("{stdout}");
                io::stdout().flush()?;
            }
            if let Some(stderr) = result["stderr"].as_str() {
                eprint!("{stderr}");
                io::stderr().flush()?;
            }
            if result["truncated"].as_bool() == Some(true) {
                eprintln!("opencoding: sandbox output was truncated at 1 MiB");
            }
            match result["exit_code"].as_i64() {
                Some(0) => Ok(()),
                Some(code) => Err(anyhow!("sandboxed command exited with {code}")),
                None => Err(anyhow!(
                    "sandboxed command terminated without a stable exit code"
                )),
            }
        }
        Some("awaiting_approval") => Err(anyhow!("sandbox approval did not resolve")),
        Some("denied") => Err(anyhow!("sandboxed command was denied by Policy")),
        Some("failed") => {
            let detail = outcome["tool_call"]["error"]
                .as_str()
                .unwrap_or("sandboxed command failed");
            let error = anyhow!(detail.to_owned());
            if detail.contains("timed out") {
                Err(error.context(CliExitStatus::Timeout))
            } else {
                Err(error)
            }
        }
        _ => Err(anyhow!("daemon returned an invalid sandbox Tool outcome")),
    }
}
