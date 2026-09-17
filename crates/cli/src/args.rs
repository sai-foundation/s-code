use anyhow::{Context, Result, anyhow};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CliCommand {
    #[default]
    Interactive,
    Exec,
    Review,
    Setup,
    Doctor,
    Completion,
    Sandbox,
    Mcp,
    Skill,
    Hook,
    Plugin,
    App,
    Client,
    Im,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OutputMode {
    #[default]
    Text,
    Jsonl,
    StreamJson,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CliArgs {
    pub(crate) command: CliCommand,
    pub(crate) prompt: Option<String>,
    pub(crate) print: bool,
    pub(crate) continue_session: bool,
    pub(crate) resume: Option<Option<String>>,
    pub(crate) model: Option<String>,
    pub(crate) permission_mode: Option<String>,
    pub(crate) output_mode: OutputMode,
    pub(crate) output_last_message: Option<PathBuf>,
    pub(crate) output_schema: Option<PathBuf>,
    pub(crate) timeout_seconds: u64,
    pub(crate) ephemeral: bool,
    pub(crate) review_target: Option<String>,
    pub(crate) setup_provider: Option<String>,
    pub(crate) setup_base_url: Option<String>,
    pub(crate) setup_credential_handle: Option<String>,
    pub(crate) completion_shell: Option<String>,
    pub(crate) sandbox_args: Vec<String>,
    pub(crate) sandbox_profile: String,
    pub(crate) sandbox_network: bool,
    pub(crate) mcp_args: Vec<String>,
    pub(crate) mcp_environment_handles: Vec<String>,
    pub(crate) mcp_header_handles: Vec<String>,
    pub(crate) mcp_oauth: bool,
    pub(crate) mcp_oauth_client_id: Option<String>,
    pub(crate) mcp_oauth_scopes: Vec<String>,
    pub(crate) mcp_timeout_ms: u64,
    pub(crate) skill_args: Vec<String>,
    pub(crate) skill_name: Option<String>,
    pub(crate) skill_description: Option<String>,
    pub(crate) skill_activation_terms: Vec<String>,
    pub(crate) skill_mcp_dependencies: Vec<String>,
    pub(crate) skill_manual_only: bool,
    pub(crate) hook_args: Vec<String>,
    pub(crate) hook_name: Option<String>,
    pub(crate) hook_modify_input: bool,
    pub(crate) plugin_args: Vec<String>,
    pub(crate) app_args: Vec<String>,
    pub(crate) client_args: Vec<String>,
    pub(crate) im_args: Vec<String>,
    pub(crate) yes: bool,
}

pub(crate) fn help() -> &'static str {
    "S-Code CLI

Usage:
  s-code [prompt]
  s-code exec [options] <prompt>
  s-code review [--uncommitted|--base <ref>|--commit <sha>]
  s-code setup [--provider <provider>] [--base-url <url>] [--model <model>] [--credential-handle <NAME>] [--yes]
  s-code doctor
  s-code sandbox [--sandbox-profile <read-only|workspace-write>] [--network] [--timeout <seconds>] [--yes] -- <program> [arg ...]
  s-code mcp list
  s-code mcp status
  s-code mcp resources <id> [cursor]
  s-code mcp templates <id> [cursor]
  s-code mcp read <id> <uri>
  s-code mcp add <id> <absolute-program> [arg ...] [--env <NAME=HANDLE>] [--timeout-ms <ms>] [--yes]
  s-code mcp add-http <id> <https-url> [--header <NAME=HANDLE> | --oauth [--oauth-client-id <id>] [--oauth-scope <scope>]] [--timeout-ms <ms>] [--yes]
  s-code mcp login <id>
  s-code mcp logout <id>
  s-code mcp remove <id> [--yes]
  s-code skill list
  s-code skill status
  s-code skill add <id> <absolute-SKILL.md> [--name <name>] --description <text> [--match <term>] [--mcp <id>] [--manual-only] [--yes]
  s-code skill enable <id>
  s-code skill disable <id>
  s-code skill remove <id> [--yes]
  s-code hook list
  s-code hook status
  s-code hook add <id> <pre-tool-use|post-tool-use> <absolute-program> [arg ...] [--name <name>] [--env <NAME=HANDLE>] [--timeout-ms <ms>] [--modify-input] [--yes]
  s-code hook remove <id> [--yes]
  s-code plugin list
  s-code plugin add <plugin>@<marketplace> [--yes]
  s-code plugin enable <plugin>@<marketplace>
  s-code plugin disable <plugin>@<marketplace>
  s-code plugin remove <plugin>@<marketplace> [--yes]
  s-code plugin marketplace list
  s-code plugin marketplace add <name> <absolute-directory-or-marketplace.json> [--yes]
  s-code plugin marketplace upgrade <name> [--yes]
  s-code plugin marketplace remove <name> [--yes]
  s-code app list
  s-code app read <id>
  s-code im telegram <connect|status|pair|approve|allow|disallow|revoke|disconnect>
  s-code client list
  s-code client revoke <client-id> [--yes]
  s-code completion <bash|zsh|fish|powershell>
  s-code -p, --print <prompt>
  s-code -c, --continue [prompt]
  s-code -r, --resume[=<session>] [prompt]

Options:
  -p, --print                 Print the response and exit
  -c, --continue              Resume the latest session in this workspace
  -r, --resume[=<session>]    Resume a session by ID or title
      --model <model>         Use a model for a new session
      --provider <provider>   Configure openrouter, openai, anthropic, gemini, local, or openai-compatible
      --base-url <url>        Configure a model API base URL
      --credential-handle <NAME>
                              Read the provider credential from this environment variable
      --permission-mode <manual|accept-edits|workspace|plan>
      --sandbox-profile <read-only|workspace-write>
      --network               Request network access for `sandbox`
      --yes                   Confirm an explicit sandbox request in automation
      --jsonl                 Emit versioned JSON Lines
      --stream-json           Emit raw versioned event JSON Lines
      --output-last-message <file>
      --output-schema <file>  Require a JSON response matching the schema
      --timeout <seconds>     Non-interactive timeout (default: 600)
      --env <NAME=HANDLE>     Map an extension environment name to a secret handle
      --header <NAME=HANDLE>  Map an MCP HTTP header to a secret handle
      --oauth                 Discover and use OAuth 2.1 with PKCE for MCP HTTP
      --oauth-client-id <id>  Use a pre-registered public OAuth client
      --oauth-scope <scope>   Request an OAuth scope; repeatable
      --timeout-ms <ms>       MCP or Hook timeout
      --name <name>           Skill or Hook display name
      --description <text>    Skill description
      --match <term>          Skill automatic activation term; repeatable
      --mcp <id>              Skill MCP dependency; repeatable
      --manual-only           Activate a Skill only through $skill-id
      --modify-input          Permit a pre-tool-use Hook to replace Tool arguments
  -y, --yes                   Confirm an extension install or removal
      --ephemeral             Delete the automation session after completion
  -h, --help                  Show help
  -V, --version               Show version

Exit codes:
  0 success · 1 runtime failure · 2 invalid usage · 124 timeout · 130 cancelled"
}

fn help_for_command(command: CliCommand) -> &'static str {
    match command {
        CliCommand::Setup => {
            "Configure the first model endpoint

Usage:
  s-code setup [options]

Options:
      --provider <provider>        openrouter, openai, anthropic, gemini, local,
                                   or openai-compatible
      --base-url <url>             Model API base URL
      --model <model>              Default model identifier
      --credential-handle <NAME>   Environment variable that contains the key
  -y, --yes                        Save without an interactive confirmation
  -h, --help                       Show this help

Provider credentials are read from the named environment variable and are
never written to the S-Code configuration file."
        }
        CliCommand::Doctor => {
            "Check the local product setup

Usage:
  s-code doctor

Checks the local service, encrypted state, model endpoint, credential handle,
workspace, and platform sandbox support."
        }
        CliCommand::Sandbox => {
            "Run one command under an explicit local sandbox profile

Usage:
  s-code sandbox [--sandbox-profile <read-only|workspace-write>] [--network]
                      [--timeout <seconds>] [--yes] -- <program> [arg ...]

Interactive use displays the command, filesystem profile and network setting
before execution. Non-interactive use must pass --yes; --network is a separate,
explicit capability request."
        }
        CliCommand::Completion => {
            "Generate shell completion

Usage:
  s-code completion <bash|zsh|fish|powershell>"
        }
        _ => help(),
    }
}

pub(crate) fn parse_args(values: impl IntoIterator<Item = String>) -> Result<Option<CliArgs>> {
    let mut args = values.into_iter().peekable();
    let mut parsed = CliArgs {
        timeout_seconds: 600,
        mcp_timeout_ms: 30_000,
        sandbox_profile: "read-only".into(),
        ..CliArgs::default()
    };
    if let Some(command) = args.peek().map(String::as_str) {
        parsed.command = match command {
            "exec" => CliCommand::Exec,
            "review" => CliCommand::Review,
            "setup" => CliCommand::Setup,
            "doctor" => CliCommand::Doctor,
            "completion" => CliCommand::Completion,
            "sandbox" => CliCommand::Sandbox,
            "mcp" => CliCommand::Mcp,
            "skill" => CliCommand::Skill,
            "hook" => CliCommand::Hook,
            "plugin" => CliCommand::Plugin,
            "app" => CliCommand::App,
            "client" => CliCommand::Client,
            "im" => CliCommand::Im,
            _ => CliCommand::Interactive,
        };
        if parsed.command != CliCommand::Interactive {
            args.next();
        }
        if matches!(parsed.command, CliCommand::Exec | CliCommand::Review) {
            parsed.print = true;
        }
    }
    let mut prompt = Vec::new();
    let mut positional = false;
    while let Some(value) = args.next() {
        if positional {
            prompt.push(value);
            continue;
        }
        match value.as_str() {
            "--" => positional = true,
            "-h" | "--help" => {
                println!("{}", help_for_command(parsed.command));
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("s-code {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--self-test" => {
                assert!(!s_code_protocol::PROTOCOL_VERSION.is_empty());
                println!("s-code CLI self-test ok");
                return Ok(None);
            }
            "-p" | "--print" => parsed.print = true,
            "-c" | "--continue" => parsed.continue_session = true,
            "-r" | "--resume" => {
                parsed.resume = Some(
                    args.next_if(|next| !next.starts_with('-'))
                        .map(Some)
                        .unwrap_or(None),
                );
            }
            "--model" => {
                parsed.model = Some(args.next().context("--model requires a value")?);
            }
            "--provider" if parsed.command == CliCommand::Setup => {
                parsed.setup_provider = Some(args.next().context("--provider requires a value")?);
            }
            "--base-url" if parsed.command == CliCommand::Setup => {
                parsed.setup_base_url = Some(args.next().context("--base-url requires a value")?);
            }
            "--credential-handle"
                if matches!(parsed.command, CliCommand::Setup | CliCommand::Im) =>
            {
                parsed.setup_credential_handle = Some(
                    args.next()
                        .context("--credential-handle requires a value")?,
                );
            }
            "--permission-mode" => {
                let mode = args.next().context("--permission-mode requires a value")?;
                if !matches!(
                    mode.as_str(),
                    "manual" | "accept-edits" | "workspace" | "plan"
                ) {
                    return Err(anyhow!(
                        "--permission-mode must be manual, accept-edits, workspace, or plan"
                    ));
                }
                parsed.permission_mode = Some(mode);
            }
            "--sandbox-profile" if parsed.command == CliCommand::Sandbox => {
                let profile = args.next().context("--sandbox-profile requires a value")?;
                if !matches!(profile.as_str(), "read-only" | "workspace-write") {
                    return Err(anyhow!(
                        "--sandbox-profile must be read-only or workspace-write"
                    ));
                }
                parsed.sandbox_profile = profile;
            }
            "--network" if parsed.command == CliCommand::Sandbox => {
                parsed.sandbox_network = true;
            }
            "--jsonl" => {
                if parsed.output_mode != OutputMode::Text {
                    return Err(anyhow!("choose only one machine output mode"));
                }
                parsed.output_mode = OutputMode::Jsonl;
                parsed.print = true;
            }
            "--stream-json" => {
                if parsed.output_mode != OutputMode::Text {
                    return Err(anyhow!("choose only one machine output mode"));
                }
                parsed.output_mode = OutputMode::StreamJson;
                parsed.print = true;
            }
            "--output-last-message" => {
                parsed.output_last_message = Some(PathBuf::from(
                    args.next()
                        .context("--output-last-message requires a file path")?,
                ));
                parsed.print = true;
            }
            "--output-schema" => {
                parsed.output_schema = Some(PathBuf::from(
                    args.next()
                        .context("--output-schema requires a file path")?,
                ));
                parsed.print = true;
            }
            "--timeout" => {
                let seconds = args
                    .next()
                    .context("--timeout requires seconds")?
                    .parse::<u64>()
                    .context("--timeout must be a positive integer")?;
                if seconds == 0 || seconds > 86_400 {
                    return Err(anyhow!("--timeout must be between 1 and 86400 seconds"));
                }
                parsed.timeout_seconds = seconds;
            }
            "--ephemeral" => {
                parsed.ephemeral = true;
                parsed.print = true;
            }
            "-y" | "--yes"
                if matches!(
                    parsed.command,
                    CliCommand::Setup
                        | CliCommand::Mcp
                        | CliCommand::Skill
                        | CliCommand::Hook
                        | CliCommand::Plugin
                        | CliCommand::Client
                        | CliCommand::Sandbox
                ) =>
            {
                parsed.yes = true
            }
            "--env" if matches!(parsed.command, CliCommand::Mcp | CliCommand::Hook) => {
                parsed
                    .mcp_environment_handles
                    .push(args.next().context("--env requires NAME=HANDLE")?);
            }
            "--header" if parsed.command == CliCommand::Mcp => {
                parsed
                    .mcp_header_handles
                    .push(args.next().context("--header requires NAME=HANDLE")?);
            }
            "--oauth" if parsed.command == CliCommand::Mcp => {
                parsed.mcp_oauth = true;
            }
            "--oauth-client-id" if parsed.command == CliCommand::Mcp => {
                parsed.mcp_oauth = true;
                parsed.mcp_oauth_client_id =
                    Some(args.next().context("--oauth-client-id requires a value")?);
            }
            "--oauth-scope" if parsed.command == CliCommand::Mcp => {
                parsed.mcp_oauth = true;
                parsed
                    .mcp_oauth_scopes
                    .push(args.next().context("--oauth-scope requires a value")?);
            }
            "--timeout-ms" if matches!(parsed.command, CliCommand::Mcp | CliCommand::Hook) => {
                let milliseconds = args
                    .next()
                    .context("--timeout-ms requires milliseconds")?
                    .parse::<u64>()
                    .context("--timeout-ms must be a positive integer")?;
                let maximum = if parsed.command == CliCommand::Hook {
                    10_000
                } else {
                    120_000
                };
                if !(100..=maximum).contains(&milliseconds) {
                    return Err(anyhow!(
                        "--timeout-ms must be between 100 and {maximum} milliseconds"
                    ));
                }
                parsed.mcp_timeout_ms = milliseconds;
            }
            "--name" if matches!(parsed.command, CliCommand::Skill | CliCommand::Hook) => {
                let name = args.next().context("--name requires a value")?;
                if parsed.command == CliCommand::Skill {
                    parsed.skill_name = Some(name);
                } else {
                    parsed.hook_name = Some(name);
                }
            }
            "--description" if parsed.command == CliCommand::Skill => {
                parsed.skill_description =
                    Some(args.next().context("--description requires a value")?);
            }
            "--match" if parsed.command == CliCommand::Skill => {
                parsed
                    .skill_activation_terms
                    .push(args.next().context("--match requires a value")?);
            }
            "--mcp" if parsed.command == CliCommand::Skill => {
                parsed
                    .skill_mcp_dependencies
                    .push(args.next().context("--mcp requires a server id")?);
            }
            "--manual-only" if parsed.command == CliCommand::Skill => {
                parsed.skill_manual_only = true;
            }
            "--modify-input" if parsed.command == CliCommand::Hook => {
                parsed.hook_modify_input = true;
            }
            "--uncommitted" if parsed.command == CliCommand::Review => {
                set_review_target(&mut parsed, "uncommitted")?;
            }
            "--base" if parsed.command == CliCommand::Review => {
                let base = args.next().context("--base requires a Git ref")?;
                set_review_target(&mut parsed, &format!("base:{base}"))?;
            }
            "--commit" if parsed.command == CliCommand::Review => {
                let commit = args.next().context("--commit requires a Git commit")?;
                set_review_target(&mut parsed, &format!("commit:{commit}"))?;
            }
            _ if value.starts_with("--resume=") => {
                let id = value.trim_start_matches("--resume=");
                if id.is_empty() {
                    return Err(anyhow!("--resume requires a non-empty session"));
                }
                parsed.resume = Some(Some(id.into()));
            }
            _ if value.starts_with('-') => return Err(anyhow!("unknown option {value}")),
            _ => prompt.push(value),
        }
    }
    if parsed.continue_session && parsed.resume.is_some() {
        return Err(anyhow!("--continue and --resume cannot be used together"));
    }
    if parsed.command == CliCommand::Mcp {
        parsed.mcp_args = prompt;
    } else if parsed.command == CliCommand::Skill {
        parsed.skill_args = prompt;
    } else if parsed.command == CliCommand::Hook {
        parsed.hook_args = prompt;
    } else if parsed.command == CliCommand::Plugin {
        parsed.plugin_args = prompt;
    } else if parsed.command == CliCommand::App {
        parsed.app_args = prompt;
    } else if parsed.command == CliCommand::Im {
        parsed.im_args = prompt;
    } else if parsed.command == CliCommand::Client {
        parsed.client_args = prompt;
    } else if parsed.command == CliCommand::Sandbox {
        parsed.sandbox_args = prompt;
    } else if !prompt.is_empty() {
        parsed.prompt = Some(prompt.join(" "));
    }
    match parsed.command {
        CliCommand::Setup if parsed.prompt.is_some() => {
            return Err(anyhow!("setup does not accept a prompt"));
        }
        CliCommand::Completion => {
            parsed.completion_shell = parsed.prompt.take();
            if parsed.completion_shell.as_deref().is_none_or(|shell| {
                shell.contains(char::is_whitespace)
                    || !matches!(shell, "bash" | "zsh" | "fish" | "powershell")
            }) {
                return Err(anyhow!(
                    "completion requires one of: bash, zsh, fish, powershell"
                ));
            }
        }
        CliCommand::Doctor if parsed.prompt.is_some() => {
            return Err(anyhow!("doctor does not accept a prompt"));
        }
        CliCommand::Sandbox if parsed.sandbox_args.is_empty() => {
            return Err(anyhow!(
                "sandbox requires `-- <program> [arg ...]`; programs resolve through the sandbox PATH"
            ));
        }
        CliCommand::Sandbox => {
            if parsed.continue_session || parsed.resume.is_some() {
                return Err(anyhow!("sandbox always uses an isolated ephemeral session"));
            }
            parsed.ephemeral = true;
        }
        CliCommand::Mcp if parsed.mcp_args.is_empty() => {
            return Err(anyhow!(
                "mcp requires one of: list, status, resources, templates, read, add, remove"
            ));
        }
        CliCommand::Skill if parsed.skill_args.is_empty() => {
            return Err(anyhow!(
                "skill requires one of: list, status, add, enable, disable, remove"
            ));
        }
        CliCommand::Hook if parsed.hook_args.is_empty() => {
            return Err(anyhow!("hook requires one of: list, status, add, remove"));
        }
        CliCommand::Plugin if parsed.plugin_args.is_empty() => {
            return Err(anyhow!(
                "plugin requires one of: list, add, enable, disable, remove, marketplace"
            ));
        }
        CliCommand::App if parsed.app_args.is_empty() => {
            return Err(anyhow!("app requires one of: list, read"));
        }
        CliCommand::Im if parsed.im_args.is_empty() => {
            return Err(anyhow!(
                "usage: s-code im telegram <connect|status|pair|approve|allow|disallow|revoke|disconnect>"
            ));
        }
        CliCommand::Client if parsed.client_args.is_empty() => {
            return Err(anyhow!("client requires one of: list, revoke"));
        }
        CliCommand::Review => {
            parsed
                .review_target
                .get_or_insert_with(|| "uncommitted".into());
            parsed.ephemeral = true;
        }
        _ => {}
    }
    Ok(Some(parsed))
}

fn set_review_target(parsed: &mut CliArgs, target: &str) -> Result<()> {
    if parsed.review_target.is_some() {
        return Err(anyhow!(
            "choose only one of --uncommitted, --base, or --commit"
        ));
    }
    parsed.review_target = Some(target.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_help_is_scoped_and_explains_secret_handling() {
        let help = help_for_command(CliCommand::Setup);
        assert!(help.contains("s-code setup [options]"));
        assert!(help.contains("never written"));
        assert!(!help.contains("s-code mcp"));
    }
}
