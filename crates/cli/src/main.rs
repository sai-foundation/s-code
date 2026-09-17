use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event as TerminalEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use s_code_config::{
    ClientConfig, Component, ConfigLoader, LocalDaemonConnection, SourceKind,
    local_daemon_instance_lock_is_free, read_local_daemon_connection,
    recorded_local_daemon_owns_instance_lock,
};
use s_code_protocol::{
    AgentResultSummary, AgentRunSummary, ApprovalScope, BackgroundTerminalSpec, ClientEvent,
    DurableTaskStatus, Id, MemoryScope, Message, PermissionMode, QuestionAnswer, QuestionStatus,
    Scope, Session, SessionGoalStatus, SessionStatus, TeamGoalRunStatus, TurnInputMode, TurnStatus,
    TurnUndoImpactPreview, UpdateSessionGoal, UpdateSessionPreferences,
};
#[cfg(test)]
use s_code_protocol::{AttachmentMetadata, TranscriptSnapshot};
use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

mod api;
mod args;
mod commands;
mod input;
mod render;
mod state;

use api::{Api, completed_tool_result, encode};
#[cfg(test)]
use args::OutputMode;
use args::{CliArgs, CliCommand, parse_args};
use commands::automation::{print_turn, validate_json_output, validate_output_schema};
use commands::completion::completion_script;
use commands::extensions::{
    mcp_permission_kind_name, run_app_command, run_client_command, run_hook_command,
    run_mcp_command, run_plugin_command, run_skill_command,
};
#[cfg(test)]
use commands::extensions::{
    parse_mcp_environment_handles, parse_mcp_header_handles, parse_plugin_selector,
};
use commands::interactive::{
    load_session_goal, load_session_preferences, parse_permission_mode, permission_mode_name,
    run_interactive_loop,
};
use commands::links::{open_external_url, transcript_links};
use commands::sandbox::run_sandbox_command;
use commands::setup::run_setup;
#[cfg(test)]
use input::paste::MAX_BRACKETED_PASTE_BYTES;
use input::{
    history::{open_history_search, recall_history},
    paste::apply_bracketed_paste,
};
#[cfg(test)]
use render::{TranscriptScrollMetrics, transcript_lines};
use render::{render, transcript_scroll_metrics};
use state::reducer::{
    apply_transcript_snapshot, merge_older_transcript_snapshot, refresh_transcript_snapshot,
};
use state::{
    App, CliTheme, Keymap, LiveEvent, PickerKind, PickerOption, PickerState, StatuslineMode,
    VimMode,
};
#[cfg(test)]
use state::{ToolActivityState, ToolProgress};

#[derive(Clone)]
struct ModelProbeTarget {
    provider: String,
    base_url: String,
    credential_handle: Option<String>,
}

fn model_probe_target(
    config: &s_code_config::ModelConfig,
    selected_model: &str,
) -> Result<ModelProbeTarget> {
    if !config.endpoints.is_empty() {
        let endpoint = config
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id == selected_model)
            .ok_or_else(|| anyhow!("selected model has no configured endpoint"))?;
        return Ok(ModelProbeTarget {
            provider: endpoint.provider.clone(),
            base_url: endpoint.base_url.clone(),
            credential_handle: endpoint.credential_handle.clone(),
        });
    }
    Ok(ModelProbeTarget {
        provider: config.provider.clone(),
        base_url: config
            .base_url
            .clone()
            .context("model API base URL is not configured")?,
        credential_handle: config.credential_handle.clone(),
    })
}

fn probe_credential_handle(target: &ModelProbeTarget) -> Option<String> {
    target.credential_handle.clone().or_else(|| {
        let loopback = url::Url::parse(&target.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .is_some_and(|host| {
                host == "localhost"
                    || host
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback())
            });
        if target.provider == "openai_compatible" && loopback {
            None
        } else {
            Some(
                match target.provider.as_str() {
                    "anthropic" => "ANTHROPIC_API_KEY",
                    "gemini" => "GEMINI_API_KEY",
                    _ => "OPENAI_API_KEY",
                }
                .into(),
            )
        }
    })
}

async fn probe_model_endpoint(
    config: &s_code_config::ModelConfig,
    selected_model: &str,
) -> Result<bool> {
    let target = model_probe_target(config, selected_model)?;
    let mut url = url::Url::parse(&target.base_url).context("model API base URL is invalid")?;
    let path = format!("{}/models", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut request = client.get(url);
    let saved = s_code_config::onboarding::path()
        .ok()
        .map(|path| s_code_config::onboarding::read(&path))
        .transpose()
        .map_err(anyhow::Error::msg)?
        .flatten()
        .filter(|saved| {
            !s_code_config::onboarding::environment_managed()
                && config.endpoints.is_empty()
                && saved.base_url == target.base_url
                && saved.provider == target.provider
        });
    let credential = if let Some(saved) = saved {
        (!saved.api_key.is_empty()).then_some(saved.api_key)
    } else if let Some(handle) = probe_credential_handle(&target) {
        Some(
            env::var(&handle)
                .with_context(|| format!("model credential handle {handle} is unavailable"))?,
        )
    } else {
        None
    };
    let credential_attached = if let Some(credential) = credential {
        if credential.trim().is_empty() {
            return Err(anyhow!("model credential is empty"));
        }
        request = match target.provider.as_str() {
            "anthropic" => request
                .header("x-api-key", credential)
                .header("anthropic-version", "2023-06-01"),
            "gemini" => request.header("x-goog-api-key", credential),
            _ => request.bearer_auth(credential),
        };
        true
    } else {
        false
    };
    let status = request
        .send()
        .await
        .context("configured model endpoint is unreachable")?
        .status();
    if !status.is_success() {
        return Err(anyhow!(
            "configured model endpoint readiness probe returned HTTP {status}"
        ));
    }
    Ok(credential_attached)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CliExitStatus {
    Usage,
    Timeout,
    Cancelled,
}

impl CliExitStatus {
    fn code(self) -> u8 {
        match self {
            Self::Usage => 2,
            Self::Timeout => 124,
            Self::Cancelled => 130,
        }
    }
}

impl std::fmt::Display for CliExitStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Usage => "invalid CLI usage",
            Self::Timeout => "operation timed out",
            Self::Cancelled => "operation cancelled",
        })
    }
}

impl std::error::Error for CliExitStatus {}

fn exit_code_for_error(error: &anyhow::Error) -> u8 {
    error
        .downcast_ref::<CliExitStatus>()
        .map_or(1, |status| status.code())
}

fn resolve_daemon_connection(
    config: &ClientConfig,
    daemon_url_is_default: bool,
    discovered: std::result::Result<LocalDaemonConnection, String>,
) -> Result<(String, String)> {
    if let Some(token) = config.token.clone() {
        return Ok((config.daemon_url.clone(), token));
    }
    let connection = discovered.map_err(|error| {
        anyhow!(
            "local service was not discovered ({error}); start `s-code web` or set S_CODE_URL and S_CODE_TOKEN"
        )
    })?;
    if !daemon_url_is_default && config.daemon_url.trim_end_matches('/') != connection.daemon_url {
        return Err(anyhow!(
            "S_CODE_URL does not match the discovered local daemon; set S_CODE_TOKEN for an explicit daemon"
        ));
    }
    Ok((connection.daemon_url, connection.token))
}

async fn load_session_state(api: &Api, app: &mut App, session_id: &Id) -> Result<()> {
    match api.transcript_snapshot(session_id).await {
        Ok(snapshot) => {
            apply_transcript_snapshot(app, snapshot);
            Ok(())
        }
        Err(_) => {
            app.clear_transcript();
            app.messages = api.messages(session_id).await?;
            app.transcript_item_order = app
                .messages
                .iter()
                .map(|message| message.id.clone())
                .collect();
            Ok(())
        }
    }
}

async fn refresh_session_state(api: &Api, app: &mut App, session_id: &Id) -> Result<()> {
    let snapshot = api.transcript_snapshot(session_id).await?;
    refresh_transcript_snapshot(app, snapshot);
    app.transcript_refresh_pending = false;
    Ok(())
}

fn tool_display(payload: &Value, tool: &str) -> String {
    payload["display"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| friendly_tool(tool))
}

fn friendly_tool(tool: &str) -> String {
    match tool {
        "execute" => "Code Mode".into(),
        "list_files" => "List files".into(),
        "read_file" => "Read file".into(),
        "search_text" => "Search code".into(),
        "apply_patch" => "Edit files".into(),
        "run_command" => "Run command".into(),
        "git_status" => "Check Git status".into(),
        "git_diff" => "Review changes".into(),
        other => other.replace('_', " "),
    }
}

fn current_workspace_uri(config: &ClientConfig) -> Result<String> {
    if let Some(workspace) = config.workspace.clone() {
        return Ok(workspace);
    }
    url::Url::from_directory_path(env::current_dir()?)
        .map(String::from)
        .map_err(|_| anyhow!("current directory cannot be represented as a file URL"))
}

fn select_start_session(
    sessions: &[Session],
    workspace: &str,
    args: &CliArgs,
) -> Result<Option<Session>> {
    if args.continue_session {
        return sessions
            .iter()
            .find(|session| {
                session.workspace_uri.trim_end_matches('/') == workspace.trim_end_matches('/')
            })
            .cloned()
            .map(Some)
            .context("no previous session exists for this workspace");
    }
    if let Some(resume) = &args.resume {
        return match resume {
            Some(query) => {
                let exact = sessions.iter().find(|session| session.id.0 == *query);
                let title = sessions
                    .iter()
                    .find(|session| session.title.to_lowercase().contains(&query.to_lowercase()));
                exact
                    .or(title)
                    .cloned()
                    .map(Some)
                    .with_context(|| format!("session {query:?} was not found"))
            }
            None => sessions
                .first()
                .cloned()
                .map(Some)
                .context("there are no sessions to resume"),
        };
    }
    Ok(None)
}

fn is_local_exit(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "exit" | "quit" | "/exit" | "/quit"
    )
}

fn confirm_sandbox_request(args: &CliArgs) -> Result<()> {
    if args.yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(anyhow!(
            "sandbox execution requires confirmation; rerun with --yes in non-interactive use"
        ));
    }
    eprintln!("Sandbox request");
    eprintln!("  profile: {}", args.sandbox_profile);
    eprintln!(
        "  network: {}",
        if args.sandbox_network {
            "enabled"
        } else {
            "disabled"
        }
    );
    eprintln!("  command: {:?}", args.sandbox_args);
    eprint!("Run this command? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Err(anyhow!("sandbox execution was not confirmed"));
    }
    if args.sandbox_network {
        eprint!("Grant this command network access? [y/N] ");
        io::stderr().flush()?;
        answer.clear();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Err(anyhow!("sandbox network access was not confirmed"));
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(exit_code_for_error(&error))
        }
    }
}

async fn run() -> Result<()> {
    let raw_args = env::args().skip(1).collect::<Vec<_>>();
    if raw_args.as_slice() == ["--local-daemon-live"] {
        let connection = read_local_daemon_connection()
            .map_err(|error| anyhow!("local daemon connection is unavailable: {error}"))?;
        let api = Api::new(
            connection.daemon_url.clone(),
            connection.token.clone(),
            Scope {
                organization_id: Id("local-health".into()),
                team_id: Id("local-health".into()),
                actor_id: Id("local-health".into()),
                goal_id: None,
                task_id: None,
            },
        );
        let health = api.health().await.context("local daemon health failed")?;
        if health.status != "ok"
            || health.development_instance_id.as_deref() != Some(connection.instance_id.as_str())
        {
            return Err(anyhow!("local daemon instance identity does not match"));
        }
        return Ok(());
    }
    if raw_args.as_slice() == ["--local-web-launch-url"] {
        let connection = read_local_daemon_connection()
            .map_err(|error| anyhow!("local daemon connection is unavailable: {error}"))?;
        let api = Api::new(
            connection.daemon_url.clone(),
            connection.token,
            Scope {
                organization_id: Id("local-browser".into()),
                team_id: Id("local-browser".into()),
                actor_id: Id("local-browser".into()),
                goal_id: None,
                task_id: None,
            },
        );
        let bootstrap = api
            .browser_bootstrap_token()
            .await
            .context("local browser bootstrap failed")?;
        println!(
            "{}/#s-code-bootstrap={bootstrap}",
            connection.daemon_url.trim_end_matches('/')
        );
        return Ok(());
    }
    if raw_args.as_slice() == ["--local-daemon-owned"] {
        if !recorded_local_daemon_owns_instance_lock()
            .map_err(|error| anyhow!("local daemon ownership is unavailable: {error}"))?
        {
            return Err(anyhow!(
                "recorded process does not own the local daemon lock"
            ));
        }
        return Ok(());
    }
    if raw_args.as_slice() == ["--local-daemon-lock-free"] {
        if !local_daemon_instance_lock_is_free()
            .map_err(|error| anyhow!("local daemon lock state is unavailable: {error}"))?
        {
            return Err(anyhow!("local daemon lock is held"));
        }
        return Ok(());
    }
    let Some(mut args) =
        parse_args(raw_args.into_iter()).map_err(|error| error.context(CliExitStatus::Usage))?
    else {
        return Ok(());
    };
    if args.command == CliCommand::Completion {
        println!(
            "{}",
            completion_script(
                args.completion_shell
                    .as_deref()
                    .context("completion shell was not parsed")?
            )
        );
        return Ok(());
    }
    if args.command == CliCommand::Setup {
        return run_setup(&args).await;
    }
    if !matches!(
        args.command,
        CliCommand::Mcp
            | CliCommand::Skill
            | CliCommand::Hook
            | CliCommand::Plugin
            | CliCommand::App
            | CliCommand::Client
            | CliCommand::Sandbox
    ) && !io::stdin().is_terminal()
    {
        let mut piped = String::new();
        io::stdin().read_to_string(&mut piped)?;
        let piped = piped.trim();
        if !piped.is_empty() {
            args.prompt = Some(match args.prompt.take() {
                Some(prompt) => format!("{piped}\n\n{prompt}"),
                None => piped.into(),
            });
            args.print = true;
        }
    }
    let plain_terminal = env::var_os("TERM").is_some_and(|value| value == "dumb");
    if !io::stdout().is_terminal() || plain_terminal {
        args.print = true;
    }
    let mut effective = ConfigLoader::from_process().load(Component::Cli)?;
    if args.command == CliCommand::Interactive
        && !args.print
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && effective.config.daemon.auth_mode == "development_token"
        && effective
            .provenance("model.base_url")
            .is_some_and(|source| source.source == SourceKind::Default)
        && effective.config.model.endpoints.is_empty()
        && effective.config.client.token.is_none()
    {
        commands::setup::run_guided_setup(&args).await?;
        effective = ConfigLoader::from_process().load(Component::Cli)?;
        if effective
            .provenance("model.base_url")
            .is_some_and(|source| source.source == SourceKind::Default)
        {
            return Ok(());
        }
    }

    let daemon_url_is_default = effective
        .provenance("client.daemon_url")
        .is_some_and(|entry| entry.source == SourceKind::Default);
    let model_config = effective.config.model.clone();
    let config = effective.config.client;
    let uses_discovered_local_daemon = config.token.is_none();
    let cli_theme = CliTheme::parse(&config.theme).expect("validated CLI theme");
    let cli_keymap = Keymap::parse(&config.keymap).expect("validated CLI keymap");
    let cli_statusline =
        StatuslineMode::parse(&config.statusline).expect("validated CLI statusline");
    let cli_editor = config.editor.clone();
    let (base, token) = resolve_daemon_connection(
        &config,
        daemon_url_is_default,
        read_local_daemon_connection(),
    )?;
    let workspace = current_workspace_uri(&config)?;
    let scope = Scope {
        organization_id: Id(config.organization_id),
        team_id: Id(config.team_id),
        actor_id: Id(config.actor_id),
        goal_id: None,
        task_id: None,
    };
    let api = if uses_discovered_local_daemon {
        Api::new_local(base, token, scope)
    } else {
        Api::new(base, token, scope)
    };
    let manifest = api.capabilities().await?;
    if !manifest.is_protocol_compatible(s_code_protocol::PROTOCOL_VERSION) {
        return Err(anyhow!(
            "incompatible daemon protocol {}; client supports {}",
            manifest.protocol_version,
            s_code_protocol::PROTOCOL_VERSION
        ));
    }
    for required in ["scope.team", "session.persistence", "event.sse_replay"] {
        if !manifest.supports(required, 1) {
            return Err(anyhow!(
                "daemon is missing required capability {required} v1"
            ));
        }
    }
    if args.command == CliCommand::Mcp {
        if !manifest.supports("extension.mcp_management", 1) {
            return Err(anyhow!(
                "daemon is missing required capability extension.mcp_management v1"
            ));
        }
        if args
            .mcp_args
            .first()
            .is_some_and(|action| matches!(action.as_str(), "resources" | "templates" | "read"))
            && !manifest.supports("mcp.resources.v1", 1)
        {
            return Err(anyhow!(
                "daemon is missing required capability mcp.resources.v1"
            ));
        }
        return run_mcp_command(&api, &args).await;
    }
    if args.command == CliCommand::Skill {
        if !manifest.supports("extension.skill_management", 1) {
            return Err(anyhow!(
                "daemon is missing required capability extension.skill_management v1"
            ));
        }
        return run_skill_command(&api, &args).await;
    }
    if args.command == CliCommand::Hook {
        if !manifest.supports("extension.hook_management", 1) {
            return Err(anyhow!(
                "daemon is missing required capability extension.hook_management v1"
            ));
        }
        return run_hook_command(&api, &args).await;
    }
    if args.command == CliCommand::Plugin {
        if !manifest.supports("extension.plugin_marketplace.v1", 1) {
            return Err(anyhow!(
                "daemon is missing required capability extension.plugin_marketplace.v1"
            ));
        }
        return run_plugin_command(&api, &args).await;
    }
    if args.command == CliCommand::App {
        if !manifest.supports("extension.plugin_app_catalog.v1", 1) {
            return Err(anyhow!(
                "daemon is missing required capability extension.plugin_app_catalog.v1"
            ));
        }
        return run_app_command(&api, &args).await;
    }
    if args.command == CliCommand::Client {
        if !manifest.supports("client.presence.v1", 1) {
            return Err(anyhow!(
                "daemon is missing required capability client.presence.v1"
            ));
        }
        return run_client_command(&api, &args).await;
    }
    if args.command == CliCommand::Sandbox && !manifest.supports("sandbox.profile.v1", 1) {
        return Err(anyhow!(
            "the daemon has no native sandbox backend on this platform; execution was not attempted"
        ));
    }
    if args.command == CliCommand::Sandbox {
        confirm_sandbox_request(&args)?;
    }
    if args.command == CliCommand::Doctor {
        let health = api.health().await.context("daemon health request failed")?;
        println!("✓ configuration loaded");
        println!("✓ workspace {}", workspace);
        println!(
            "✓ daemon {}",
            if health.status == "ok" {
                "healthy"
            } else {
                health.status.as_str()
            }
        );
        println!(
            "✓ protocol {} · {} enabled capabilities",
            manifest.protocol_version,
            manifest
                .capabilities
                .iter()
                .filter(|capability| capability.enabled)
                .count()
        );
        match health.storage_protection.as_str() {
            "managed_encrypted" => println!("✓ storage encrypted with a private managed key"),
            "explicit_encrypted" => println!("✓ storage encrypted with an explicit key"),
            "ephemeral_memory" => println!("✓ storage is ephemeral memory"),
            "legacy_plaintext" => {
                println!(
                    "✗ storage uses a legacy plaintext database · preserve it, then select a fresh state directory before sensitive work"
                );
            }
            other => println!("! storage protection is {other}"),
        }
        let mut model_endpoint_ready = false;
        if health.model_provider_configured && health.model_credentials_available {
            match probe_model_endpoint(&model_config, &config.model).await {
                Ok(true) => {
                    model_endpoint_ready = true;
                    println!("✓ model endpoint catalog reachable; credential available");
                }
                Ok(false) => {
                    model_endpoint_ready = true;
                    println!("✓ local model endpoint catalog reachable; no credential required");
                }
                Err(error) => println!("✗ model endpoint readiness failed · {error}"),
            }
        } else if health.model_provider_configured {
            println!(
                "✗ model credential unavailable · export the handle selected by s-code setup, then restart S-Code"
            );
        } else {
            println!("✗ model endpoint unavailable · run s-code setup");
        }
        if health.status != "ok" {
            return Err(anyhow!("daemon health check returned {}", health.status));
        }
        if health.storage_protection == "legacy_plaintext" {
            return Err(anyhow!(
                "legacy plaintext storage requires remediation before sensitive use"
            ));
        }
        if !health.model_provider_configured {
            return Err(anyhow!("model endpoint is not configured"));
        }
        if !health.model_credentials_available {
            return Err(anyhow!("model credential handle is unavailable"));
        }
        if !model_endpoint_ready {
            return Err(anyhow!("model endpoint readiness check failed"));
        }
        return Ok(());
    }
    let agent_enabled = manifest.supports("agent.tool_loop", 1);
    let undo_enabled = manifest.supports("turn.undo", 1);
    let mut sessions = api.sessions().await?;
    let selected = if args.ephemeral {
        None
    } else {
        select_start_session(&sessions, &workspace, &args)?
    };
    let selected = if let Some(session) = selected {
        session
    } else {
        api.create_session(workspace, args.model.clone().unwrap_or(config.model))
            .await?
    };
    if !sessions.iter().any(|session| session.id == selected.id) {
        sessions.insert(0, selected.clone());
    }
    if let Some(index) = sessions
        .iter()
        .position(|session| session.id == selected.id)
    {
        sessions.swap(0, index);
    }
    if let Some(mode) = args.permission_mode.as_deref() {
        let permission_mode = parse_permission_mode(mode)
            .ok_or_else(|| anyhow!("unknown permission mode {mode:?}"))?;
        api.update_session_preferences(
            &selected.id,
            UpdateSessionPreferences {
                scope: api.scope.clone(),
                permission_mode: Some(permission_mode),
                assistant_alias: None,
            },
        )
        .await?;
    }
    if args.command == CliCommand::Sandbox {
        let program = args
            .sandbox_args
            .first()
            .context("sandbox program was not parsed")?;
        let result = run_sandbox_command(
            &api,
            &selected.id,
            program,
            &args.sandbox_args[1..],
            &args.sandbox_profile,
            args.sandbox_network,
            args.timeout_seconds,
        )
        .await;
        let cleanup = api.delete_session(&selected.id).await.map(|_| ());
        result?;
        cleanup.context("failed to delete ephemeral sandbox session")?;
        return Ok(());
    }
    if args.print {
        let mut prompt = if args.command == CliCommand::Review {
            args.prompt.take().unwrap_or_default()
        } else {
            args.prompt
                .take()
                .context("--print and non-interactive terminals require a prompt")?
        };
        if args.permission_mode.as_deref() == Some("plan") {
            return Err(anyhow!(
                "--permission-mode plan cannot execute a prompt in --print mode"
            ));
        }
        let output_schema = if let Some(path) = args.output_schema.as_ref() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("failed to read output schema {}", path.display()))?;
            let schema: Value = serde_json::from_str(&raw)
                .with_context(|| format!("output schema {} is not valid JSON", path.display()))?;
            validate_output_schema(&schema)
                .with_context(|| format!("invalid output schema {}", path.display()))?;
            prompt.push_str(
                "\n\nReturn only JSON matching this output schema. Do not wrap it in Markdown:\n",
            );
            prompt.push_str(&serde_json::to_string(&schema)?);
            Some(schema)
        } else {
            None
        };
        let turn = if args.command == CliCommand::Review {
            api.start_review(
                &selected.id,
                args.review_target
                    .clone()
                    .unwrap_or_else(|| "uncommitted".into()),
                (!prompt.trim().is_empty()).then_some(prompt),
            )
            .await?
        } else {
            api.start_turn(&selected.id, prompt, vec![], !args.ephemeral)
                .await?
        };
        let result = tokio::time::timeout(
            Duration::from_secs(args.timeout_seconds),
            print_turn(&api, &selected.id, &turn, args.output_mode),
        )
        .await
        .map_err(|_| {
            anyhow!("turn timed out after {} seconds", args.timeout_seconds)
                .context(CliExitStatus::Timeout)
        });
        let cleanup = if args.ephemeral {
            api.delete_session(&selected.id).await.map(|_| ())
        } else {
            Ok(())
        };
        let answer = result??;
        cleanup.context("failed to delete ephemeral session")?;
        if let Some(schema) = output_schema {
            validate_json_output(&answer, &schema)?;
        }
        if let Some(path) = args.output_last_message {
            fs::write(&path, &answer)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        return Ok(());
    }
    let mut app = App::new(sessions, agent_enabled, undo_enabled);
    app.configure_interface(cli_theme, cli_keymap, cli_statusline, cli_editor);
    if let Some(session_id) = app.current().map(|session| session.id.clone()) {
        load_session_state(&api, &mut app, &session_id).await?;
        load_session_preferences(&api, &mut app, &session_id).await;
        load_session_goal(&api, &mut app, &session_id).await;
    }
    if let Some(prompt) = args.prompt {
        let session = app.current().context("no active session")?.id.clone();
        app.prompt_history.push(prompt.clone());
        let turn = api.start_turn(&session, prompt, vec![], true).await?;
        load_session_state(&api, &mut app, &session).await?;
        app.current_turn = Some(turn);
        app.turn_running = true;
        app.status = "starting".into();
    }
    if args.permission_mode.is_some() {
        app.status = format!(
            "permission mode: {}",
            permission_mode_name(&app.permission_mode)
        );
    }
    run_interactive_loop(&api, &mut app, &manifest).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::interactive::*;
    use ratatui::backend::TestBackend;

    fn discovered_connection() -> LocalDaemonConnection {
        LocalDaemonConnection::new(
            "http://127.0.0.1:4567".into(),
            "discovered-token".into(),
            "instance-0123456789".into(),
        )
    }

    fn session() -> Session {
        serde_json::from_value(json!({
            "id":"ses_1",
            "scope":{"organization_id":"org","team_id":"team","actor_id":"user","goal_id":null,"task_id":null},
            "workspace_uri":"file:///workspace",
            "title":"Team coding session",
            "model":"model",
            "status":"active",
            "created_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn osc52_copy_is_bounded_and_encodes_the_exact_answer() {
        let mut output = Vec::new();
        write_osc52(&mut output, "你好, S-Code").unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            format!(
                "\u{1b}]52;c;{}\u{7}",
                STANDARD.encode("你好, S-Code".as_bytes())
            )
        );
        assert!(write_osc52(Vec::new(), "").is_err());
        assert!(
            write_osc52(Vec::new(), &"x".repeat(MAX_CLIPBOARD_BYTES + 1))
                .unwrap_err()
                .to_string()
                .contains("100 KiB")
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_editor_round_trip_is_shell_free_bounded_and_cleans_its_draft() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.work/cli-tests")
            .join(format!(
                "editor-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        fs::create_dir_all(&directory).unwrap();
        let edited = edit_with_external_editor(
            "sh -c 'printf \"%s\" \"edited in external editor\" > \"$1\"' s-code-editor",
            "initial",
            Some(&directory),
        )
        .unwrap();
        assert_eq!(edited, "edited in external editor");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir(&directory).unwrap();
    }

    #[test]
    fn vim_keymap_has_real_insert_normal_motion_and_command_modes() {
        let mut app = App::new(Vec::new(), true, true);
        app.keymap = Keymap::Vim;
        app.input.insert_str("abc");

        assert!(handle_vim_key(&mut app, KeyCode::Esc));
        assert_eq!(app.vim_mode, VimMode::Normal);
        assert!(handle_vim_key(&mut app, KeyCode::Char('h')));
        assert!(handle_vim_key(&mut app, KeyCode::Char('x')));
        assert_eq!(app.input.as_str(), "ab");
        assert!(handle_vim_key(&mut app, KeyCode::Char(':')));
        assert_eq!(app.vim_mode, VimMode::Insert);
        assert_eq!(app.input.as_str(), "/");
        assert!(!handle_vim_key(&mut app, KeyCode::Char('t')));
    }

    #[tokio::test]
    async fn interface_slash_commands_change_live_cli_state() {
        let api = Api::new(
            "http://127.0.0.1:1".into(),
            "unused".into(),
            Scope {
                organization_id: Id("org".into()),
                team_id: Id("team".into()),
                actor_id: Id("user".into()),
                goal_id: None,
                task_id: None,
            },
        );
        let mut app = App::new(Vec::new(), true, true);

        run_command(&api, &mut app, "/theme light").await;
        run_command(&api, &mut app, "/statusline compact").await;
        run_command(&api, &mut app, "/keymap vim").await;
        assert_eq!(app.theme, CliTheme::Light);
        assert_eq!(app.statusline, StatuslineMode::Compact);
        assert_eq!(app.keymap, Keymap::Vim);
        assert_eq!(app.vim_mode, VimMode::Normal);

        run_command(&api, &mut app, "/vim").await;
        assert_eq!(app.keymap, Keymap::Emacs);
        run_command(&api, &mut app, "/editor hello draft").await;
        assert_eq!(app.pending_editor.as_deref(), Some("hello draft"));
        app.usage.input_tokens = 12;
        app.usage.output_tokens = 3;
        app.usage.total_tokens = 15;
        app.usage.model_calls = 2;
        app.usage.tool_calls = 1;
        app.usage.turns = 1;
        run_command(&api, &mut app, "/usage").await;
        assert!(app.tool_result.contains("Tokens       15"));
        assert!(app.tool_result.contains("Model calls  2"));
        assert_eq!(app.status, "session usage · 15 tokens · 1 turns");

        app.messages = vec![message(
            "msg_link",
            "turn_link",
            "assistant",
            "[Guide](https://docs.example.com/guide) and [unsafe](file:///etc/passwd)",
        )];
        run_command(&api, &mut app, "/links guide").await;
        let picker = app.picker.as_ref().unwrap();
        assert_eq!(picker.kind, PickerKind::Link);
        assert_eq!(picker.options.len(), 1);
        assert_eq!(picker.options[0].label, "Guide");
        assert_eq!(picker.options[0].id, "https://docs.example.com/guide");
        assert_eq!(picker.query, "guide");
    }

    fn attachment(id: &str, turn_id: Option<&str>, file_name: &str) -> AttachmentMetadata {
        serde_json::from_value(json!({
            "id": id,
            "session_id": "ses_1",
            "turn_id": turn_id,
            "file_name": file_name,
            "media_type": "text/plain",
            "byte_length": 42,
            "sha256": "0123456789abcdef",
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn automatically_uses_discovered_local_daemon() {
        let config = ClientConfig::default();
        let resolved =
            resolve_daemon_connection(&config, true, Ok(discovered_connection())).unwrap();
        assert_eq!(
            resolved,
            ("http://127.0.0.1:4567".into(), "discovered-token".into())
        );
    }

    #[test]
    fn explicit_connection_requires_its_own_token() {
        let mut config = ClientConfig {
            daemon_url: "http://127.0.0.1:9999".into(),
            ..ClientConfig::default()
        };
        assert!(
            resolve_daemon_connection(&config, false, Ok(discovered_connection()))
                .unwrap_err()
                .to_string()
                .contains("does not match")
        );
        config.token = Some("explicit-token".into());
        assert_eq!(
            resolve_daemon_connection(&config, false, Err("ignored".into())).unwrap(),
            ("http://127.0.0.1:9999".into(), "explicit-token".into())
        );
    }

    #[test]
    fn parses_complete_sse_and_keeps_partial_tail() {
        let mut buffer = concat!(
            "id: 7\n",
            "event: model.delta\n",
            "data: {\"id\":\"event_7\",\"sequence\":7,\"timestamp\":\"2026-01-01T00:00:00Z\",",
            "\"session_id\":\"session_1\",\"turn_id\":\"turn_1\",\"item_id\":\"item_1\",",
            "\"request_id\":null,\"type\":\"model.delta\",\"status\":\"streaming\",",
            "\"payload_version\":1,\"notification\":{\"type\":\"agent_message_delta\",",
            "\"item_id\":\"item_1\",\"delta\":\"hi\"},\"payload\":{\"text\":\"hi\"}}\n\n",
            "id: 8\n"
        )
        .as_bytes()
        .to_vec();
        let events = drain_sse(&mut buffer).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, 7);
        assert_eq!(events[0].payload["text"], "hi");
        assert_eq!(events[0].item_id, Some(Id("item_1".into())));
        assert_eq!(buffer, b"id: 8\n");
    }

    #[test]
    fn extracts_completed_tool_result() {
        let result = completed_tool_result(json!({
            "outcome": "completed",
            "tool_call": {"result": {"unified_diff": "diff --git a/tracked.txt"}}
        }))
        .unwrap();
        assert_eq!(result["unified_diff"], "diff --git a/tracked.txt");
    }

    #[test]
    fn maps_common_attachment_extensions_without_guessing_unknown_files() {
        assert_eq!(
            attachment_media_type(std::path::Path::new("notes.md")),
            "text/plain"
        );
        assert_eq!(
            attachment_media_type(std::path::Path::new("diagram.PNG")),
            "image/png"
        );
        assert_eq!(
            attachment_media_type(std::path::Path::new("report.pdf")),
            "application/pdf"
        );
        assert_eq!(
            attachment_media_type(std::path::Path::new("archive.unknown")),
            "application/octet-stream"
        );
    }

    #[test]
    fn searchable_picker_filters_wraps_and_renders_policy_lock_reasons() {
        let mut picker = PickerState {
            kind: PickerKind::Model,
            title: "Choose model".into(),
            options: vec![
                PickerOption {
                    id: "provider/fast".into(),
                    label: "Fast".into(),
                    detail: "provider · recommended".into(),
                    disabled_reason: None,
                },
                PickerOption {
                    id: "provider/locked".into(),
                    label: "Locked".into(),
                    detail: "provider".into(),
                    disabled_reason: Some("Blocked by Team policy".into()),
                },
            ],
            query: "lock".into(),
            selected: 0,
        };
        assert_eq!(picker.selected_option().unwrap().id, "provider/locked");
        picker.move_selection(1);
        assert_eq!(picker.selected, 0);

        let mut app = App::new(vec![session()], true, true);
        app.picker = Some(picker);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Model picker"));
        assert!(rendered.contains("Blocked by Team policy"));
        assert!(!rendered.contains("recommended"));
        assert_eq!(
            parse_permission_mode("accept-edits"),
            Some(PermissionMode::AcceptEdits)
        );
        assert_eq!(
            parse_permission_mode("workspace"),
            Some(PermissionMode::Workspace)
        );
    }

    #[test]
    fn renders_consumed_and_pending_attachments_with_names_and_actions() {
        let mut app = App::new(vec![session()], true, true);
        app.messages = vec![message(
            "msg_1",
            "turn_1",
            "user",
            "Please inspect the files.",
        )];
        app.message_attachments.insert(
            Id("msg_1".into()),
            vec![attachment("att_1", Some("turn_1"), "requirements.md")],
        );
        app.pending_attachments
            .push(attachment("att_2", None, "diagram.png"));

        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("requirements.md"));
        assert!(rendered.contains("diagram.png"));
        assert!(rendered.contains("/detach 1"));
    }

    #[test]
    fn rejects_non_completed_or_missing_tool_results() {
        assert!(completed_tool_result(json!({"outcome": "awaiting_approval"})).is_err());
        assert!(
            completed_tool_result(json!({
                "outcome": "completed",
                "tool_call": {"result": null}
            }))
            .is_err()
        );
    }

    #[test]
    fn session_updated_event_renames_the_shared_session() {
        let mut app = App::new(vec![session()], true, true);
        app.apply_event(LiveEvent {
            id: 1,
            timestamp: chrono::Utc::now(),
            kind: "session.updated".into(),
            payload: json!({"title":"统一配置设计"}),
            session_id: Some(Id("ses_1".into())),
            turn_id: None,
            item_id: None,
        });
        assert_eq!(app.sessions[0].title, "统一配置设计");
    }

    fn message(id: &str, turn_id: &str, role: &str, content: &str) -> Message {
        serde_json::from_value(json!({
            "id": id,
            "session_id": "ses_1",
            "turn_id": turn_id,
            "role": role,
            "content": content,
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    }

    fn event(id: u64, turn_id: &str, kind: &str, payload: Value) -> LiveEvent {
        LiveEvent {
            id,
            timestamp: chrono::Utc::now(),
            kind: kind.into(),
            payload,
            session_id: None,
            turn_id: Some(Id(turn_id.into())),
            item_id: None,
        }
    }

    fn completed_turn() -> s_code_protocol::Turn {
        serde_json::from_value(json!({
            "id":"turn_1",
            "session_id":"ses_1",
            "scope":{
                "organization_id":"org",
                "team_id":"team",
                "actor_id":"user",
                "goal_id":null,
                "task_id":null
            },
            "status":"completed",
            "checkpoint":null,
            "error_code":null,
            "started_at":"2026-01-01T00:00:00Z",
            "updated_at":"2026-01-01T00:00:01Z",
            "completed_at":"2026-01-01T00:00:01Z"
        }))
        .unwrap()
    }

    fn transcript_message_item(
        id: &str,
        content: &str,
        created_at: &str,
    ) -> s_code_protocol::TranscriptItem {
        serde_json::from_value(json!({
            "id":id,
            "session_id":"ses_1",
            "turn_id":"turn_1",
            "kind":"agent_message",
            "status":"completed",
            "created_at":created_at,
            "started_at":created_at,
            "completed_at":created_at,
            "summary":content,
            "content":{"type":"message","role":"assistant","content":content,"attachments":[]},
            "detail":null,
            "approval_id":null,
            "policy_id":null,
            "audit_event_id":null,
            "truncated":false,
            "retryable":false,
            "cancellable":false,
            "capability_version":"1",
            "revision":1
        }))
        .unwrap()
    }

    fn transcript_snapshot(
        items: Vec<s_code_protocol::TranscriptItem>,
        next_cursor: Option<String>,
        cursor: u64,
        item_count: u64,
        usage: s_code_protocol::SessionUsage,
    ) -> TranscriptSnapshot {
        TranscriptSnapshot {
            protocol_version: "1.0.0".into(),
            snapshot_revision: cursor,
            session: session(),
            turns: vec![completed_turn()],
            item_count,
            next_cursor,
            pending_requests: Vec::new(),
            pending_questions: Vec::new(),
            pending_inputs: Vec::new(),
            attachments: Vec::new(),
            artifacts: Vec::new(),
            items,
            usage,
            cursor,
        }
    }

    #[test]
    fn streamed_messages_use_the_snapshot_message_rendering_path_and_ignore_replay() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.turn_running = true;
        let mut first = event(1, "turn_1", "model.delta", json!({"text":"hello"}));
        first.session_id = Some(Id("ses_1".into()));
        first.item_id = Some(Id("item_1".into()));
        let mut tool = event(
            2,
            "turn_1",
            "tool.completed",
            json!({
                "tool_call_id":"tool_1",
                "tool":"list_files",
                "display":"List files · ."
            }),
        );
        tool.session_id = Some(Id("ses_1".into()));
        tool.item_id = Some(Id("tool_1".into()));
        let mut second = event(3, "turn_1", "model.delta", json!({"text":" world"}));
        second.session_id = Some(Id("ses_1".into()));
        second.item_id = Some(Id("item_1".into()));

        app.apply_event(first);
        app.apply_event(tool);
        app.apply_event(second.clone());
        app.apply_event(second);

        assert_eq!(app.messages.len(), 1);
        assert_eq!(app.messages[0].id, Id("item_1".into()));
        assert_eq!(app.messages[0].content, json!("hello world"));
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("hello world"));
        assert_eq!(rendered.matches("hello world").count(), 1);
        assert!(
            rendered.find("List files · .").expect("tool should render")
                < rendered.find("hello world").expect("answer should render")
        );
        assert_eq!(app.event_cursor, 3);
    }

    #[test]
    fn global_sequence_gap_is_accepted_in_a_team_filtered_stream() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.turn_running = true;
        assert!(app.apply_event(event(
            7,
            "turn_1",
            "turn.status",
            json!({"status":"calling_model"})
        )));
        assert!(app.apply_event(event(
            9,
            "turn_1",
            "model.delta",
            json!({"text":"lost-prefix"})
        )));

        assert_eq!(app.event_cursor, 9);
        assert_eq!(app.messages[0].content, json!("lost-prefix"));
        assert!(!app.status.contains("event gap"));
    }

    #[test]
    fn model_delta_offset_gap_is_rejected_without_consuming_the_event() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.turn_running = true;
        let mut first = event(
            1,
            "turn_1",
            "model.delta",
            json!({"text":"你好", "byte_offset":0}),
        );
        first.session_id = Some(Id("ses_1".into()));
        first.item_id = Some(Id("item_1".into()));
        assert!(app.apply_event(first));
        let mut gap = event(
            2,
            "turn_1",
            "model.delta",
            json!({"text":"!", "byte_offset":5}),
        );
        gap.session_id = Some(Id("ses_1".into()));
        gap.item_id = Some(Id("item_1".into()));

        assert!(!app.apply_event(gap));
        assert_eq!(app.event_cursor, 1);
        assert_eq!(app.messages[0].content, json!("你好"));
        assert!(app.status.contains("expected byte offset 6"));
    }

    #[test]
    fn satisfies_the_shared_cli_web_cursor_conformance_cases() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/cases/transcript-reducer-conformance.json"
        ))
        .expect("shared transcript conformance fixture should parse");
        assert_eq!(fixture["schema_version"], 1);
        for case in fixture["cases"]
            .as_array()
            .expect("conformance cases should be an array")
        {
            let mut app = App::new(vec![session()], true, true);
            app.event_cursor = case["initial_cursor"].as_u64().unwrap();
            let session_id = case["session_id"].as_str().unwrap();
            let mut event = event(
                case["sequence"].as_u64().unwrap(),
                "turn_1",
                "turn.status",
                json!({"status":"calling_model"}),
            );
            event.session_id = Some(Id(session_id.into()));
            let accepted = app.apply_event(event);
            assert_eq!(
                accepted,
                case["accepted"].as_bool().unwrap(),
                "{}",
                case["name"].as_str().unwrap()
            );
            assert_eq!(
                app.event_cursor,
                case["expected_cursor"].as_u64().unwrap(),
                "{}",
                case["name"].as_str().unwrap()
            );
            let visible = accepted && session_id == "ses_1";
            assert_eq!(
                visible,
                case["visible"].as_bool().unwrap(),
                "{}",
                case["name"].as_str().unwrap()
            );
            assert!(
                !app.status.contains("event gap"),
                "{}",
                case["name"].as_str().unwrap()
            );
        }
        for case in fixture["append_cases"]
            .as_array()
            .expect("append conformance cases should be an array")
        {
            let mut app = App::new(vec![session()], true, true);
            app.event_cursor = case["initial_cursor"].as_u64().unwrap();
            app.current_turn = Some(Id("turn_1".into()));
            app.turn_running = true;
            app.messages.push(Message {
                id: Id("item-a".into()),
                session_id: Id("ses_1".into()),
                turn_id: Id("turn_1".into()),
                role: "assistant".into(),
                content: case["initial_text"].clone(),
                created_at: chrono::Utc::now(),
            });
            let mut delta = event(
                case["sequence"].as_u64().unwrap(),
                "turn_1",
                "model.delta",
                json!({
                    "text": case["delta"],
                    "byte_offset": case["byte_offset"],
                }),
            );
            delta.session_id = Some(Id("ses_1".into()));
            delta.item_id = Some(Id("item-a".into()));
            let accepted = app.apply_event(delta);
            assert_eq!(
                accepted,
                case["accepted"].as_bool().unwrap(),
                "{}",
                case["name"].as_str().unwrap()
            );
            assert_eq!(app.event_cursor, case["expected_cursor"].as_u64().unwrap());
            assert_eq!(app.messages[0].content, case["expected_text"]);
            if let Some(expected) = case["expected_append_gap"].as_object() {
                assert!(app.status.contains(&format!(
                    "expected byte offset {}",
                    expected["expected"].as_u64().unwrap()
                )));
                assert!(app.status.contains(&format!(
                    "received {}",
                    expected["received"].as_u64().unwrap()
                )));
            }
        }
    }

    #[test]
    fn structured_plan_updates_one_visible_item_in_its_turn() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        let mut first = event(
            1,
            "turn_1",
            "plan.updated",
            json!({
                "item_id":"plan_turn_1",
                "title":"Implementation",
                "steps":[
                    {"text":"Inspect","status":"completed"},
                    {"text":"Test","status":"in_progress"}
                ]
            }),
        );
        first.session_id = Some(Id("ses_1".into()));
        first.item_id = Some(Id("plan_turn_1".into()));
        app.apply_event(first);
        let mut second = event(
            2,
            "turn_1",
            "plan.updated",
            json!({
                "item_id":"plan_turn_1",
                "title":"Implementation",
                "steps":[
                    {"text":"Inspect","status":"completed"},
                    {"text":"Test","status":"completed"}
                ]
            }),
        );
        second.session_id = Some(Id("ses_1".into()));
        second.item_id = Some(Id("plan_turn_1".into()));
        app.apply_event(second);
        assert_eq!(app.plans.len(), 1);
        assert_eq!(app.plans[0].steps.len(), 2);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Plan · Implementation"));
        assert!(rendered.contains("✓ Test"));
    }

    #[test]
    fn context_compaction_and_model_reroute_render_inline_in_their_turn() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.messages = vec![message("msg_1", "turn_1", "assistant", "Done")];
        let mut compacted = event(
            1,
            "turn_1",
            "context.compacted",
            json!({
                "item_id":"context-one",
                "omitted_messages":12,
                "truncated_messages":1,
                "estimated_tokens":480
            }),
        );
        compacted.session_id = Some(Id("ses_1".into()));
        compacted.item_id = Some(Id("context-one".into()));
        app.apply_event(compacted);
        let mut rerouted = event(
            2,
            "turn_1",
            "model.route.fallback",
            json!({
                "from_model_id":"primary",
                "to_model_id":"fallback",
                "fallback_reason":"rate limited"
            }),
        );
        rerouted.session_id = Some(Id("ses_1".into()));
        app.apply_event(rerouted);

        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(app.notices.len(), 2);
        assert!(rendered.contains("Context optimized"));
        assert!(rendered.contains("12 earlier messages summarized"));
        assert!(rendered.contains("primary → fallback"));
        assert!(rendered.contains("rate limited"));
    }

    #[test]
    fn reasoning_summary_deltas_update_one_inline_notice() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.messages = vec![message("msg_1", "turn_1", "assistant", "Done")];
        for (sequence, text) in [(1, "Checked "), (2, "the constraints.")] {
            let mut reasoning = event(
                sequence,
                "turn_1",
                "reasoning.summary.delta",
                json!({"item_id":"reasoning-one","text":text}),
            );
            reasoning.session_id = Some(Id("ses_1".into()));
            reasoning.item_id = Some(Id("reasoning-one".into()));
            app.apply_event(reasoning);
        }
        assert_eq!(
            app.notices
                .iter()
                .filter(|notice| notice.item_id.0 == "reasoning-one")
                .count(),
            1
        );
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Reasoning summary"));
        assert!(rendered.contains("Checked the constraints."));
        assert!(app.tool_activity.is_empty());
    }

    #[test]
    fn usage_event_updates_exact_session_totals_and_renders_once() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.messages = vec![message("msg_1", "turn_1", "assistant", "Done")];
        let mut usage = event(
            1,
            "turn_1",
            "turn.usage",
            json!({
                "item_id":"usage-one",
                "model":"model-a",
                "input_units":12,
                "output_units":3,
                "model_calls":2,
                "tool_calls":1
            }),
        );
        usage.session_id = Some(Id("ses_1".into()));
        usage.item_id = Some(Id("usage-one".into()));
        app.apply_event(usage.clone());
        app.apply_event(usage);

        assert_eq!(app.usage.total_tokens, 15);
        assert_eq!(app.usage.turns, 1);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(rendered.matches("15 tokens").count(), 1);
        assert!(rendered.contains("model-a"));
    }

    #[test]
    fn same_turn_messages_tools_and_usage_follow_transcript_item_order() {
        let mut app = App::new(vec![session()], true, true);
        app.messages = vec![
            message("user-one", "turn_1", "user", "check the weather"),
            message(
                "assistant-one",
                "turn_1",
                "assistant",
                "I will check from the command line.",
            ),
            message(
                "assistant-two",
                "turn_1",
                "assistant",
                "Tomorrow will be warm.",
            ),
        ];
        app.tool_activity.push(state::ToolActivity {
            parent_tool_call_id: None,
            item_id: Id("tool-one".into()),
            turn_id: Some(Id("turn_1".into())),
            call_id: Some("tool-one".into()),
            tool: "run_command".into(),
            display: "Run command · curl weather.example".into(),
            state: ToolActivityState::Completed,
            progress: None,
        });
        app.notices.extend([
            state::NoticeActivity {
                item_id: Id("usage-one".into()),
                turn_id: Id("turn_1".into()),
                label: "Usage".into(),
                detail: "100 tokens".into(),
            },
            state::NoticeActivity {
                item_id: Id("usage-two".into()),
                turn_id: Id("turn_1".into()),
                label: "Usage".into(),
                detail: "200 tokens".into(),
            },
        ]);
        app.transcript_item_order = [
            "user-one",
            "assistant-one",
            "usage-one",
            "tool-one",
            "assistant-two",
            "usage-two",
        ]
        .into_iter()
        .map(|id| Id(id.into()))
        .collect();

        let text = transcript_lines(&app)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let position = |needle: &str| {
            text.iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} was not rendered"))
        };

        assert!(position("check the weather") < position("I will check"));
        assert!(position("I will check") < position("100 tokens"));
        assert!(position("100 tokens") < position("curl weather.example"));
        assert!(position("curl weather.example") < position("Tomorrow will be warm"));
        assert!(position("Tomorrow will be warm") < position("200 tokens"));
    }

    #[test]
    fn older_transcript_page_prepends_items_without_advancing_the_live_cursor() {
        let mut app = App::new(vec![session()], true, true);
        apply_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![transcript_message_item(
                    "newer",
                    "newer answer",
                    "2026-01-01T00:00:02Z",
                )],
                Some("older-cursor".into()),
                10,
                2,
                s_code_protocol::SessionUsage {
                    input_tokens: 12,
                    output_tokens: 3,
                    total_tokens: 15,
                    model_calls: 1,
                    tool_calls: 0,
                    turns: 1,
                },
            ),
        );
        merge_older_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![transcript_message_item(
                    "older",
                    "older answer",
                    "2026-01-01T00:00:01Z",
                )],
                None,
                99,
                2,
                s_code_protocol::SessionUsage {
                    input_tokens: 20,
                    output_tokens: 5,
                    total_tokens: 25,
                    model_calls: 2,
                    tool_calls: 0,
                    turns: 2,
                },
            ),
        );

        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["older", "newer"]
        );
        assert_eq!(
            app.transcript_item_order
                .iter()
                .map(|item_id| item_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["older", "newer"]
        );
        assert_eq!(app.event_cursor, 10);
        assert_eq!(app.usage.total_tokens, 15);
        assert_eq!(app.transcript_loaded_items, 2);
        assert_eq!(app.transcript_item_count, 2);
        assert!(app.transcript_next_cursor.is_none());
    }

    #[test]
    fn current_session_refresh_replaces_the_latest_window_and_preserves_older_history() {
        let mut app = App::new(vec![session()], true, true);
        apply_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![
                    transcript_message_item(
                        "overlap",
                        "answer before refresh",
                        "2026-01-01T00:00:02Z",
                    ),
                    transcript_message_item("stale", "remove me", "2026-01-01T00:00:03Z"),
                ],
                Some("older-page".into()),
                10,
                4,
                s_code_protocol::SessionUsage::default(),
            ),
        );
        merge_older_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![transcript_message_item(
                    "older",
                    "already paged history",
                    "2026-01-01T00:00:01Z",
                )],
                Some("oldest-page".into()),
                9,
                4,
                s_code_protocol::SessionUsage::default(),
            ),
        );
        app.transcript_viewport = state::TranscriptViewport::Detached { top_row: 7 };

        refresh_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![
                    transcript_message_item(
                        "overlap",
                        "answer after refresh",
                        "2026-01-01T00:00:02Z",
                    ),
                    transcript_message_item("new", "new canonical item", "2026-01-01T00:00:04Z"),
                ],
                Some("snapshot-page".into()),
                20,
                4,
                s_code_protocol::SessionUsage {
                    output_tokens: 12,
                    total_tokens: 12,
                    turns: 1,
                    ..s_code_protocol::SessionUsage::default()
                },
            ),
        );

        assert_eq!(
            app.transcript_item_order
                .iter()
                .map(|item_id| item_id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["older", "overlap", "new"]
        );
        assert_eq!(
            app.messages
                .iter()
                .map(|message| message.content.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "already paged history",
                "answer after refresh",
                "new canonical item"
            ]
        );
        assert_eq!(app.transcript_next_cursor.as_deref(), Some("oldest-page"));
        assert_eq!(app.transcript_loaded_items, 3);
        assert_eq!(app.transcript_item_count, 4);
        assert_eq!(app.event_cursor, 20);
        assert_eq!(app.usage.total_tokens, 12);
        assert_eq!(
            app.transcript_viewport,
            state::TranscriptViewport::Detached { top_row: 7 }
        );
    }

    #[test]
    fn authoritative_snapshot_replacement_resets_history_and_viewport() {
        let mut app = App::new(vec![session()], true, true);
        app.messages = vec![message("old", "turn_1", "assistant", "old session state")];
        app.transcript_item_order = vec![Id("old".into())];
        app.transcript_viewport = state::TranscriptViewport::Detached { top_row: 3 };
        app.transcript_refresh_pending = true;

        apply_transcript_snapshot(
            &mut app,
            transcript_snapshot(
                vec![transcript_message_item(
                    "replacement",
                    "canonical state",
                    "2026-01-01T00:00:02Z",
                )],
                None,
                30,
                1,
                s_code_protocol::SessionUsage::default(),
            ),
        );

        assert_eq!(app.transcript_item_order, vec![Id("replacement".into())]);
        assert!(app.transcript_follows_tail());
        assert!(!app.transcript_refresh_pending);
    }

    #[test]
    fn structured_question_is_inline_and_not_an_unnamed_tool_row() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.apply_event(event(
            1,
            "turn_1",
            "tool.proposed",
            json!({"model_call_id":"model-question","tool":"request_user_input"}),
        ));
        let mut required = event(
            2,
            "turn_1",
            "question.required",
            json!({
                "request_id":"question-one",
                "item_id":"question-item-one",
                "allow_other":true,
                "questions":[{
                    "id":"approach",
                    "header":"Approach",
                    "question":"Which implementation should I use?",
                    "options":[
                        {"label":"Fast","description":"Ship the smallest safe change."},
                        {"label":"Complete","description":"Build the full workflow."}
                    ]
                }]
            }),
        );
        required.item_id = Some(Id("question-item-one".into()));
        app.apply_event(required);
        assert!(app.tool_activity.is_empty());
        assert_eq!(app.questions.len(), 1);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Answer needed · question-one"));
        assert!(rendered.contains("Which implementation should I use?"));
        assert!(rendered.contains("1. Fast"));
        assert!(rendered.contains("/answer question-one"));
    }

    #[test]
    fn artifact_is_inline_and_not_an_unnamed_tool_row() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.apply_event(event(
            1,
            "turn_1",
            "tool.proposed",
            json!({"model_call_id":"model-artifact","tool":"publish_artifact"}),
        ));
        let mut created = event(
            2,
            "turn_1",
            "artifact.created",
            json!({
                "item_id":"artifact-item-one",
                "artifact_id":"artifact-one",
                "title":"Test report",
                "media_type":"text/markdown"
            }),
        );
        created.item_id = Some(Id("artifact-item-one".into()));
        app.apply_event(created);
        assert!(app.tool_activity.is_empty());
        assert_eq!(app.artifacts.len(), 1);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Artifact · Test report"));
        assert!(rendered.contains("/artifact artifact-one"));
    }

    #[test]
    fn durable_pending_input_is_visible_and_server_created_turn_becomes_current() {
        let mut app = App::new(vec![session()], true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.turn_running = true;
        app.pending_inputs.push_back(
            serde_json::from_value(json!({
                "id":"input-one",
                "session_id":"ses_1",
                "target_turn_id":"turn_1",
                "resulting_turn_id":null,
                "scope":{"organization_id":"org","team_id":"team","actor_id":"user","goal_id":null,"task_id":null},
                "mode":"queue",
                "content":"Check the edge case next",
                "status":"pending",
                "idempotency_key":"cli-input-one",
                "created_at":"2026-01-01T00:00:00Z",
                "updated_at":"2026-01-01T00:00:00Z",
                "consumed_at":null,
                "cancelled_at":null,
                "revision":1
            }))
            .unwrap(),
        );
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Queued · Check the edge case next"));
        assert!(rendered.contains("/dequeue input-one"));

        let mut created = event(1, "turn_2", "turn.created", json!({"status":"idle"}));
        created.session_id = Some(Id("ses_1".into()));
        app.apply_event(created);
        assert_eq!(app.current_turn, Some(Id("turn_2".into())));
        assert!(app.turn_running);
        assert_eq!(app.status, "starting");
    }

    #[test]
    fn merges_one_tool_lifecycle_into_one_named_row() {
        let mut app = App::new(Vec::new(), true, true);
        app.apply_event(event(
            1,
            "turn_1",
            "tool.proposed",
            json!({"model_call_id":"model_1","tool":"run_command"}),
        ));
        app.apply_event(event(
            2,
            "turn_1",
            "approval.required",
            json!({
                "approval_id":"approval_1",
                "tool_call_id":"tool_1",
                "tool":"run_command",
                "display":"Run command · cargo test"
            }),
        ));
        app.apply_event(event(
            3,
            "turn_1",
            "tool.completed",
            json!({
                "tool_call_id":"tool_1",
                "tool":"run_command",
                "display":"Run command · cargo test"
            }),
        ));

        assert_eq!(app.tool_activity.len(), 1);
        assert_eq!(app.tool_activity[0].display, "Run command · cargo test");
        assert_eq!(app.tool_activity[0].state, ToolActivityState::Completed);
        assert!(app.approvals.is_empty());
    }

    #[test]
    fn mcp_progress_updates_the_existing_named_tool_row() {
        let mut app = App::new(Vec::new(), true, true);
        app.current_turn = Some(Id("turn_1".into()));
        app.apply_event(event(
            1,
            "turn_1",
            "tool.running",
            json!({
                "tool_call_id":"tool_1",
                "tool":"mcp.fixture.echo",
                "display":"MCP fixture · echo"
            }),
        ));
        app.apply_event(event(
            2,
            "turn_1",
            "mcp.progress",
            json!({
                "item_id":"tool_1",
                "tool_call_id":"tool_1",
                "server":"fixture",
                "tool":"echo",
                "progress":25,
                "total":100,
                "message":"Preparing fixture"
            }),
        ));

        assert_eq!(app.tool_activity.len(), 1);
        assert_eq!(
            app.tool_activity[0].progress,
            Some(ToolProgress {
                progress: 25.0,
                total: Some(100.0),
                message: Some("Preparing fixture".into()),
            })
        );
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("MCP fixture · echo"));
        assert!(rendered.contains("25% · Preparing fixture"));
    }

    #[test]
    fn renders_tools_inside_their_own_question_instead_of_at_the_bottom() {
        let mut app = App::new(Vec::new(), true, true);
        app.messages = vec![
            message("msg_1", "turn_1", "user", "first question"),
            message("msg_2", "turn_1", "assistant", "first answer"),
            message("msg_3", "turn_2", "user", "second question"),
            message("msg_4", "turn_2", "assistant", "second answer"),
        ];
        app.apply_event(event(
            1,
            "turn_1",
            "tool.completed",
            json!({"tool_call_id":"tool_1","tool":"read_file","display":"Read file · src/a.rs"}),
        ));
        app.apply_event(event(
            2,
            "turn_2",
            "tool.completed",
            json!({"tool_call_id":"tool_2","tool":"run_command","display":"Run command · cargo test"}),
        ));

        let text = transcript_lines(&app)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let position = |needle: &str| {
            text.iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} was not rendered"))
        };

        assert!(position("first question") < position("Read file · src/a.rs"));
        assert!(position("Read file · src/a.rs") < position("first answer"));
        assert!(position("second question") < position("Run command · cargo test"));
        assert!(position("Run command · cargo test") < position("second answer"));
    }

    #[test]
    fn parses_non_interactive_and_resume_arguments() {
        let parsed = parse_args(
            ["-p", "--resume=ses_1", "--model", "model-a", "fix", "tests"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert!(parsed.print);
        assert_eq!(parsed.resume, Some(Some("ses_1".into())));
        assert_eq!(parsed.model.as_deref(), Some("model-a"));
        assert_eq!(parsed.prompt.as_deref(), Some("fix tests"));
    }

    #[test]
    fn starts_fresh_by_default_and_resumes_only_when_requested() {
        let existing = session();
        let sessions = vec![existing.clone()];
        let workspace = "file:///workspace";

        assert!(
            select_start_session(&sessions, workspace, &CliArgs::default())
                .unwrap()
                .is_none()
        );

        let continued = CliArgs {
            continue_session: true,
            ..CliArgs::default()
        };
        assert_eq!(
            select_start_session(&sessions, workspace, &continued)
                .unwrap()
                .unwrap()
                .id,
            existing.id
        );

        let resumed = CliArgs {
            resume: Some(Some("Team coding".into())),
            ..CliArgs::default()
        };
        assert_eq!(
            select_start_session(&sessions, workspace, &resumed)
                .unwrap()
                .unwrap()
                .id,
            existing.id
        );
    }

    #[test]
    fn exact_exit_inputs_are_local_commands() {
        for value in ["exit", " EXIT ", "quit", "/exit", "/QUIT"] {
            assert!(is_local_exit(value), "{value:?} should exit locally");
        }
        for value in ["exit the loop", "how do I quit?", "/exit now", ""] {
            assert!(!is_local_exit(value), "{value:?} should remain a prompt");
        }
    }

    #[test]
    fn rejects_conflicting_or_unsafe_argument_modes() {
        assert!(parse_args(["--continue", "--resume"].into_iter().map(str::to_owned)).is_err());
        assert!(
            parse_args(
                ["--permission-mode", "unrestricted"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn parses_exec_and_review_as_explicit_automation_surfaces() {
        let exec = parse_args(
            [
                "exec",
                "--jsonl",
                "--timeout",
                "45",
                "--ephemeral",
                "fix",
                "tests",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(exec.command, CliCommand::Exec);
        assert_eq!(exec.output_mode, OutputMode::Jsonl);
        assert_eq!(exec.timeout_seconds, 45);
        assert!(exec.ephemeral);
        assert_eq!(exec.prompt.as_deref(), Some("fix tests"));

        let review = parse_args(
            ["review", "--base", "main", "focus", "on", "auth"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(review.command, CliCommand::Review);
        assert!(review.ephemeral);
        assert_eq!(review.review_target.as_deref(), Some("base:main"));
        assert_eq!(review.prompt.as_deref(), Some("focus on auth"));
        assert!(
            parse_args(
                ["review", "--base", "main", "--commit", "abc"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );

        let sandbox = parse_args(
            [
                "sandbox",
                "--sandbox-profile",
                "workspace-write",
                "--network",
                "--yes",
                "--timeout",
                "30",
                "--",
                "cargo",
                "test",
                "--locked",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(sandbox.command, CliCommand::Sandbox);
        assert_eq!(sandbox.sandbox_profile, "workspace-write");
        assert!(sandbox.sandbox_network);
        assert!(sandbox.yes);
        assert_eq!(sandbox.timeout_seconds, 30);
        assert_eq!(sandbox.sandbox_args, ["cargo", "test", "--locked"]);
        assert!(sandbox.ephemeral);
        assert!(
            parse_args(
                ["sandbox", "--sandbox-profile", "unrestricted", "--", "true"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn parses_setup_as_a_daemon_independent_first_run_surface() {
        let setup = parse_args(
            [
                "setup",
                "--provider",
                "openai-compatible",
                "--base-url",
                "https://models.example/v1",
                "--credential-handle",
                "MODEL_API_KEY",
                "--model",
                "example/model",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(setup.command, CliCommand::Setup);
        assert_eq!(setup.setup_provider.as_deref(), Some("openai-compatible"));
        assert_eq!(
            setup.setup_base_url.as_deref(),
            Some("https://models.example/v1")
        );
        assert_eq!(
            setup.setup_credential_handle.as_deref(),
            Some("MODEL_API_KEY")
        );
        assert_eq!(setup.model.as_deref(), Some("example/model"));
        assert!(setup.yes);
        assert!(
            parse_args(
                ["setup", "--provider", "local", "unexpected-prompt"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn completion_and_output_schema_contracts_are_deterministic() {
        let completion = parse_args(["completion", "zsh"].into_iter().map(str::to_owned))
            .unwrap()
            .unwrap();
        assert_eq!(completion.command, CliCommand::Completion);
        assert!(completion_script("zsh").contains("#compdef s-code"));
        assert!(completion_script("zsh").contains("mcp:manage MCP servers"));
        assert!(completion_script("zsh").contains("skill:manage Skills"));
        assert!(completion_script("zsh").contains("hook:manage Hooks"));
        assert!(completion_script("zsh").contains("sandbox:run in the product sandbox"));
        assert!(parse_args(["completion", "unknown"].into_iter().map(str::to_owned)).is_err());

        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["result", "items"],
            "additionalProperties": false,
            "properties": {
                "result": {"enum": ["ok", "failed"]},
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["score"],
                        "properties": {"score": {"type": "integer", "minimum": 0}}
                    }
                }
            }
        });
        validate_json_output(r#"{"result":"ok","items":[{"score":2}]}"#, &schema).unwrap();
        assert!(
            validate_json_output(r#"{"result":"other","items":[{"score":2}]}"#, &schema).is_err()
        );
        assert!(
            validate_json_output(r#"{"result":"ok","items":[{"score":-1}]}"#, &schema).is_err()
        );
        assert!(
            validate_json_output(
                r#"{"result":"ok","items":[{"score":2}],"unexpected":true}"#,
                &schema
            )
            .is_err()
        );
        assert!(validate_json_output("not-json", &schema).is_err());
        assert!(validate_output_schema(&json!({"type":"not-a-json-schema-type"})).is_err());
        assert!(
            validate_json_output(
                r#"{"result":"ok"}"#,
                &json!({"$ref":"https://schemas.example.invalid/result.json"})
            )
            .is_err()
        );
    }

    #[test]
    fn automation_exit_codes_are_stable_and_typed() {
        assert_eq!(exit_code_for_error(&anyhow!("runtime failure")), 1);
        assert_eq!(
            exit_code_for_error(&anyhow!("bad flag").context(CliExitStatus::Usage)),
            2
        );
        assert_eq!(
            exit_code_for_error(&anyhow!("deadline reached").context(CliExitStatus::Timeout)),
            124
        );
        assert_eq!(
            exit_code_for_error(&anyhow!("turn cancelled").context(CliExitStatus::Cancelled)),
            130
        );
    }

    #[test]
    fn bracketed_paste_is_bounded_multiline_and_never_submits() {
        let mut app = App::new(vec![], true, true);
        apply_bracketed_paste(&mut app, "first line\nsecond line");
        assert_eq!(app.input.as_str(), "first line\nsecond line");
        assert_eq!(app.status, "pasted input inserted without submitting");
        assert!(app.prompt_history.is_empty());

        let oversized = "界".repeat(MAX_BRACKETED_PASTE_BYTES / 3 + 10);
        app.input.clear();
        apply_bracketed_paste(&mut app, &oversized);
        assert!(app.input.as_str().len() <= MAX_BRACKETED_PASTE_BYTES);
        assert!(
            app.input
                .as_str()
                .is_char_boundary(app.input.as_str().len())
        );
        assert!(app.status.contains("capped"));
    }

    #[test]
    fn mouse_wheel_scrolls_transcript_and_clamps_to_loaded_rows() {
        let mut app = App::new(vec![], true, true);

        assert!(
            handle_transcript_mouse_scroll(&mut app, MouseEventKind::ScrollUp, |_| Ok(
                TranscriptScrollMetrics {
                    max_scroll: 5,
                    page_rows: 4,
                }
            ))
            .unwrap()
        );
        assert_eq!(app.transcript_rows_from_bottom(5), 3);
        assert!(
            handle_transcript_mouse_scroll(&mut app, MouseEventKind::ScrollUp, |_| Ok(
                TranscriptScrollMetrics {
                    max_scroll: 5,
                    page_rows: 4,
                }
            ))
            .unwrap()
        );
        assert_eq!(app.transcript_rows_from_bottom(5), 5);
        assert!(
            handle_transcript_mouse_scroll(&mut app, MouseEventKind::ScrollDown, |_| Ok(
                TranscriptScrollMetrics {
                    max_scroll: 5,
                    page_rows: 4,
                }
            ))
            .unwrap()
        );
        assert_eq!(app.transcript_rows_from_bottom(5), 2);

        let measurements = std::cell::Cell::new(0);
        assert!(
            !handle_transcript_mouse_scroll(&mut app, MouseEventKind::Moved, |_| {
                measurements.set(measurements.get() + 1);
                Ok(TranscriptScrollMetrics {
                    max_scroll: 5,
                    page_rows: 4,
                })
            })
            .unwrap()
        );
        assert_eq!(measurements.get(), 0);
    }

    #[test]
    fn scroll_down_uses_the_current_maximum_after_reflow() {
        let mut app = App::new(vec![], true, true);
        app.scroll_transcript_up(111, 111);

        app.scroll_transcript_down(3, 27);

        assert_eq!(app.transcript_top_row(27), 3);
        assert_eq!(app.transcript_rows_from_bottom(27), 24);
    }

    #[test]
    fn resize_clamps_a_detached_viewport_without_following_new_output() {
        let mut app = App::new(vec![], true, true);
        app.scroll_transcript_up(3, 111);

        app.clamp_transcript_viewport(27);

        assert_eq!(app.transcript_top_row(27), 27);
        assert_eq!(app.transcript_top_row(28), 27);
        assert!(!app.transcript_follows_tail());
    }

    #[test]
    fn reverse_history_search_filters_newest_unique_prompts() {
        let mut app = App::new(vec![], true, true);
        app.prompt_history = vec![
            "first task".into(),
            "multi-line\nsecond detail".into(),
            "first task".into(),
        ];
        open_history_search(&mut app);
        let picker = app.picker.as_mut().unwrap();
        assert_eq!(picker.kind, PickerKind::History);
        assert_eq!(picker.options.len(), 2);
        assert_eq!(picker.options[0].id, "first task");
        assert_eq!(picker.options[1].id, "multi-line\nsecond detail");
        picker.query = "second".into();
        assert_eq!(
            picker.selected_option().unwrap().id,
            "multi-line\nsecond detail"
        );

        apply_bracketed_paste(&mut app, " detail\nquery");
        assert_eq!(app.picker.as_ref().unwrap().query, "second detail query");
    }

    #[test]
    fn parses_mcp_management_without_treating_server_arguments_as_a_prompt() {
        let add = parse_args(
            [
                "mcp",
                "add",
                "context",
                "/usr/bin/env",
                "node",
                "server.js",
                "--env",
                "API_TOKEN=CONTEXT_API_TOKEN",
                "--timeout-ms",
                "45000",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(add.command, CliCommand::Mcp);
        assert_eq!(
            add.mcp_args,
            ["add", "context", "/usr/bin/env", "node", "server.js"]
        );
        assert_eq!(add.mcp_environment_handles, ["API_TOKEN=CONTEXT_API_TOKEN"]);
        assert_eq!(add.mcp_timeout_ms, 45_000);
        assert!(add.yes);
        assert_eq!(
            parse_mcp_environment_handles(&add.mcp_environment_handles)
                .unwrap()
                .get("API_TOKEN")
                .map(String::as_str),
            Some("CONTEXT_API_TOKEN")
        );
        let add_http = parse_args(
            [
                "mcp",
                "add-http",
                "remote",
                "https://mcp.example.test/mcp",
                "--header",
                "Authorization=REMOTE_MCP_ACCESS_TOKEN",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            add_http.mcp_args,
            ["add-http", "remote", "https://mcp.example.test/mcp"]
        );
        assert_eq!(
            parse_mcp_header_handles(&add_http.mcp_header_handles)
                .unwrap()
                .get("Authorization")
                .map(String::as_str),
            Some("REMOTE_MCP_ACCESS_TOKEN")
        );
        let add_oauth = parse_args(
            [
                "mcp",
                "add-http",
                "remote-oauth",
                "https://mcp.example.test/mcp",
                "--oauth",
                "--oauth-client-id",
                "s-code-public",
                "--oauth-scope",
                "mcp:tools",
                "--oauth-scope",
                "mcp:resources",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert!(add_oauth.mcp_oauth);
        assert_eq!(
            add_oauth.mcp_oauth_client_id.as_deref(),
            Some("s-code-public")
        );
        assert_eq!(add_oauth.mcp_oauth_scopes, ["mcp:tools", "mcp:resources"]);
        assert_eq!(
            parse_args(
                ["mcp", "login", "remote-oauth", "--yes"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .unwrap()
            .unwrap()
            .mcp_args,
            ["login", "remote-oauth"]
        );

        let remove = parse_args(
            ["mcp", "remove", "context", "-y"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(remove.mcp_args, ["remove", "context"]);
        assert!(remove.yes);
        let resource = parse_args(
            ["mcp", "read", "context", "docs://project/readme"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            resource.mcp_args,
            ["read", "context", "docs://project/readme"]
        );
        let next_page = parse_args(
            ["mcp", "resources", "context", "opaque-cursor"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            next_page.mcp_args,
            ["resources", "context", "opaque-cursor"]
        );
        assert!(parse_args(["mcp"].into_iter().map(str::to_owned)).is_err());
        assert!(
            parse_args(
                ["mcp", "add", "server", "/bin/tool", "--timeout-ms", "99"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .is_err()
        );
        assert!(parse_mcp_environment_handles(&["TOKEN".into()]).is_err());
        assert!(parse_mcp_environment_handles(&["TOKEN=A".into(), "TOKEN=B".into()]).is_err());
        assert!(parse_mcp_header_handles(&["Bad Header=TOKEN".into()]).is_err());
        let revoke_client = parse_args(
            ["client", "revoke", "web:remote-client", "--yes"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(revoke_client.client_args, ["revoke", "web:remote-client"]);
        assert!(revoke_client.yes);
    }

    #[test]
    fn parses_plugin_marketplace_and_app_lifecycle_commands() {
        let marketplace = parse_args(
            [
                "plugin",
                "marketplace",
                "add",
                "local",
                "/opt/plugins",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(marketplace.command, CliCommand::Plugin);
        assert_eq!(
            marketplace.plugin_args,
            ["marketplace", "add", "local", "/opt/plugins"]
        );
        assert!(marketplace.yes);

        let plugin = parse_args(
            ["plugin", "update", "review-pack@local", "-y"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(plugin.plugin_args, ["update", "review-pack@local"]);
        assert!(plugin.yes);
        assert_eq!(
            parse_plugin_selector("review-pack@local").unwrap(),
            ("review-pack", "local")
        );
        assert!(parse_plugin_selector("missing-marketplace").is_err());
        assert!(parse_plugin_selector("../unsafe@local").is_err());

        let app = parse_args(["app", "read", "review-app"].into_iter().map(str::to_owned))
            .unwrap()
            .unwrap();
        assert_eq!(app.command, CliCommand::App);
        assert_eq!(app.app_args, ["read", "review-app"]);
        assert!(parse_args(["plugin"].into_iter().map(str::to_owned)).is_err());
        assert!(parse_args(["app"].into_iter().map(str::to_owned)).is_err());
    }

    #[test]
    fn parses_hook_management_without_treating_handler_arguments_as_a_prompt() {
        let add = parse_args(
            [
                "hook",
                "add",
                "validate-edits",
                "pre-tool-use",
                "/bin/sh",
                "hook.sh",
                "--name",
                "Validate edits",
                "--modify-input",
                "--env",
                "RULES_TOKEN=VALIDATION_RULES_TOKEN",
                "--timeout-ms",
                "2500",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(add.command, CliCommand::Hook);
        assert_eq!(
            add.hook_args,
            [
                "add",
                "validate-edits",
                "pre-tool-use",
                "/bin/sh",
                "hook.sh"
            ]
        );
        assert_eq!(add.hook_name.as_deref(), Some("Validate edits"));
        assert!(add.hook_modify_input);
        assert_eq!(
            parse_mcp_environment_handles(&add.mcp_environment_handles)
                .unwrap()
                .get("RULES_TOKEN")
                .map(String::as_str),
            Some("VALIDATION_RULES_TOKEN")
        );
        assert_eq!(add.mcp_timeout_ms, 2500);
        assert!(add.yes);

        let remove = parse_args(
            ["hook", "remove", "validate-edits", "-y"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(remove.hook_args, ["remove", "validate-edits"]);
        assert!(remove.yes);
        assert!(parse_args(["hook"].into_iter().map(str::to_owned)).is_err());
        assert!(
            parse_args(
                [
                    "hook",
                    "add",
                    "observe",
                    "post-tool-use",
                    "/bin/sh",
                    "--timeout-ms",
                    "10001"
                ]
                .into_iter()
                .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn parses_skill_management_with_repeatable_matching_and_dependency_options() {
        let add = parse_args(
            [
                "skill",
                "add",
                "secure-review",
                "/opt/skills/secure-review/SKILL.md",
                "--name",
                "Secure review",
                "--description",
                "Review authorization changes",
                "--match",
                "authorization review",
                "--match",
                "security review",
                "--mcp",
                "repository",
                "--yes",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert_eq!(add.command, CliCommand::Skill);
        assert_eq!(
            add.skill_args,
            ["add", "secure-review", "/opt/skills/secure-review/SKILL.md"]
        );
        assert_eq!(add.skill_name.as_deref(), Some("Secure review"));
        assert_eq!(
            add.skill_description.as_deref(),
            Some("Review authorization changes")
        );
        assert_eq!(
            add.skill_activation_terms,
            ["authorization review", "security review"]
        );
        assert_eq!(add.skill_mcp_dependencies, ["repository"]);
        assert!(!add.skill_manual_only);
        assert!(add.yes);

        let manual = parse_args(
            [
                "skill",
                "add",
                "release",
                "/opt/skills/release/SKILL.md",
                "--description",
                "Prepare a release",
                "--manual-only",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
        .unwrap();
        assert!(manual.skill_manual_only);
        assert!(manual.skill_activation_terms.is_empty());
        assert_eq!(
            parse_args(
                ["skill", "disable", "secure-review"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .unwrap()
            .unwrap()
            .skill_args,
            ["disable", "secure-review"]
        );
        assert!(parse_args(["skill"].into_iter().map(str::to_owned)).is_err());
    }

    #[test]
    fn agent_result_renderer_includes_bounded_content_and_costs() {
        let result: AgentResultSummary = serde_json::from_value(json!({
            "agent_id": "agent-one",
            "parent_id": null,
            "session_id": "session-one",
            "status": "succeeded",
            "final_message_id": "message-one",
            "summary": "Focused tests passed.\nNo regressions found.",
            "summary_byte_length": 42,
            "truncated": true,
            "artifact_count": 2,
            "consumed_cost_micros": 125000,
            "consumed_runner_cost_micros": 25000,
            "completed_at": "2026-07-27T00:00:00Z"
        }))
        .unwrap();
        let rendered = render_agent_results(&[result]);
        assert!(rendered.contains("agent-one  Succeeded  session session-one"));
        assert!(rendered.contains("model $0.1250 · runner $0.0250"));
        assert!(rendered.contains("2 artifact(s) · truncated from 42 bytes"));
        assert!(rendered.contains("  Focused tests passed."));
        assert!(rendered.contains("  No regressions found."));
    }

    #[test]
    fn persistent_goal_is_rendered_above_the_composer() {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(vec![session()], true, true);
        app.goal = Some(
            serde_json::from_value(json!({
                "id": "session-goal-one",
                "session_id": "ses_1",
                "scope": {
                    "organization_id": "org",
                    "team_id": "team",
                    "actor_id": "user",
                    "goal_id": null,
                    "task_id": null
                },
                "objective": "Ship a verified Goal flow",
                "status": "active",
                "auto_continue": true,
                "token_budget": 1000,
                "input_tokens": 120,
                "output_tokens": 80,
                "continuation_count": 2,
                "last_turn_id": "turn-one",
                "blocked_reason": null,
                "created_at": "2026-07-27T00:00:00Z",
                "updated_at": "2026-07-27T00:00:00Z",
                "completed_at": null,
                "revision": 1
            }))
            .unwrap(),
        );
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Goal active"));
        assert!(rendered.contains("Ship a verified Goal flow"));
        assert!(rendered.contains("200 / 1000 tokens"));
    }

    #[test]
    fn slash_commands_render_in_a_selectable_panel_above_the_composer() {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(vec![session()], true, true);
        app.input.replace("/arch");
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Commands · ↑↓ select · Enter/Tab complete · Esc close"));
        assert!(rendered.contains("/archive"));
        assert!(rendered.contains("/unarchive"));
        assert!(rendered.contains("Restore an archived Session"));
        assert!(
            rendered.find("Commands ·").expect("command panel")
                < rendered.find("Message S-Code").expect("composer")
        );
    }

    #[test]
    fn approval_panel_is_fail_closed_and_keyboard_selectable() {
        let backend = TestBackend::new(120, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(vec![session()], true, true);
        app.approvals.push_back(state::ApprovalRequest {
            id: "approval-one".into(),
            turn_id: Some(Id("turn-one".into())),
            tool: "run_command".into(),
            display: "Run command · curl 'wttr.in?m&1&q'".into(),
        });

        assert_eq!(
            selected_approval_decision(&app),
            (false, ApprovalScope::Once)
        );
        move_approval_selection(&mut app, -1);
        assert_eq!(
            selected_approval_decision(&app),
            (true, ApprovalScope::Once)
        );
        move_approval_selection(&mut app, 1);
        assert_eq!(
            selected_approval_decision(&app),
            (false, ApprovalScope::Once)
        );
        move_approval_selection(&mut app, 1);
        assert_eq!(
            selected_approval_decision(&app),
            (true, ApprovalScope::Once)
        );

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("[1] Allow once"));
        assert!(rendered.contains("[2] Reject"));
        assert!(rendered.contains("←/→ select · Enter confirm · 1/2 choose directly"));
    }

    #[test]
    fn single_column_layout_draws_at_supported_widths() {
        for width in [80, 120, 160] {
            let backend = TestBackend::new(width, 32);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = App::new(Vec::new(), true, true);
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(rendered.contains("s-code"));
            assert!(rendered.contains("What are we building?"));
            assert!(!rendered.contains("Activity"));
            assert!(!rendered.contains("Team dashboard"));
        }
    }
    #[test]
    fn code_mode_parallel_children_keep_identity_and_final_cancelled_state() {
        let mut app = App::new(Vec::new(), true, true);
        for (sequence, id, path) in [(1, "child-a", "a.rs"), (2, "child-b", "b.rs")] {
            app.apply_event(event(sequence,"turn_1","tool.running",json!({"tool_call_id":id,"parent_tool_call_id":"program","tool":"read_file","display":path})));
        }
        assert_eq!(app.tool_activity.len(), 2);
        app.apply_event(event(
            3,
            "turn_1",
            "tool.denied",
            json!({"tool_call_id":"child-a","parent_tool_call_id":"program","tool":"read_file"}),
        ));
        app.apply_event(event(
            4,
            "turn_1",
            "tool.proposed",
            json!({"model_call_id":"direct-model","tool":"read_file"}),
        ));
        app.apply_event(event(
            5,
            "turn_1",
            "tool.running",
            json!({"tool_call_id":"direct","tool":"read_file"}),
        ));
        assert_eq!(app.tool_activity.len(), 3);
        assert!(app.tool_activity[2].parent_tool_call_id.is_none());
        assert_eq!(app.tool_activity[0].state, ToolActivityState::Denied);
        app.apply_event(event(
            6,
            "turn_1",
            "tool.cancelled",
            json!({"tool_call_id":"child-b","parent_tool_call_id":"program","tool":"read_file"}),
        ));
        app.apply_event(event(
            7,
            "turn_1",
            "tool.completed",
            json!({"tool_call_id":"child-b","parent_tool_call_id":"program","tool":"read_file"}),
        ));
        assert_eq!(app.tool_activity[1].state, ToolActivityState::Cancelled);
    }
    #[test]
    fn code_mode_snapshot_keeps_parent_and_renders_cancellation() {
        let item=serde_json::from_value(json!({
            "id":"child","session_id":"ses_1","turn_id":"turn_1","kind":"file_read","status":"cancelled",
            "created_at":"2026-09-13T00:00:00Z","started_at":null,"completed_at":null,"summary":"Read file · a.rs",
            "content":{"type":"tool_call","tool_call_id":"child","parent_tool_call_id":"program","tool":"read_file","display":"Read file · a.rs","policy_reason":"allowed","result_summary":"Cancelled"},
            "detail":null,"approval_id":null,"policy_id":null,"audit_event_id":null,"truncated":false,"retryable":false,"cancellable":false,"capability_version":"1","revision":2
        })).unwrap();
        let snapshot = TranscriptSnapshot {
            protocol_version: "1.0.0".into(),
            snapshot_revision: 10,
            session: session(),
            turns: vec![],
            items: vec![item],
            item_count: 1,
            next_cursor: None,
            pending_requests: vec![],
            pending_questions: vec![],
            pending_inputs: vec![],
            attachments: vec![],
            artifacts: vec![],
            usage: Default::default(),
            cursor: 10,
        };
        let mut app = App::new(vec![session()], true, true);
        apply_transcript_snapshot(&mut app, snapshot);
        assert_eq!(
            app.tool_activity[0].parent_tool_call_id.as_deref(),
            Some("program")
        );
        assert_eq!(app.tool_activity[0].state, ToolActivityState::Cancelled);
        let rendered = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Code Mode › Read file · a.rs cancelled"));
    }
}
