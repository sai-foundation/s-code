use crate::{api::Api, args::CliArgs, commands::links::open_external_url};
use anyhow::{Context, Result, anyhow};
use opencoding_protocol::{
    ClientPresence, ExtensionDescriptor, ExtensionKind, ExtensionPermission,
    ExtensionPermissionKind, ExtensionStatus, HookEvent, HookSpec, MarketplaceSource,
    MarketplaceSourceKind, McpHttpServerSpec, McpOAuthSpec, McpServerSpec, SkillSpec,
};
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn mcp_status_name(status: &ExtensionStatus) -> &'static str {
    match status {
        ExtensionStatus::Available => "available",
        ExtensionStatus::Installed => "installed",
        ExtensionStatus::Connected => "connected",
        ExtensionStatus::Disabled => "disabled",
        ExtensionStatus::NeedsAuthentication => "needs-authentication",
        ExtensionStatus::Locked => "locked",
        ExtensionStatus::Failed => "failed",
    }
}

pub(crate) fn mcp_permission_kind_name(kind: &ExtensionPermissionKind) -> &'static str {
    match kind {
        ExtensionPermissionKind::Tool => "tool",
        ExtensionPermissionKind::File => "file",
        ExtensionPermissionKind::Network => "network",
        ExtensionPermissionKind::Secret => "secret",
        ExtensionPermissionKind::Command => "command",
        ExtensionPermissionKind::Data => "data",
    }
}

fn print_mcp_permissions(permissions: &[ExtensionPermission]) {
    println!("Permissions:");
    for permission in permissions {
        println!(
            "  {}  {}",
            mcp_permission_kind_name(&permission.kind),
            permission.value
        );
        println!("      {}", permission.reason);
    }
}

fn confirm_mcp_change(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(anyhow!(
            "{prompt} requires confirmation; rerun with --yes in non-interactive use"
        ));
    }
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

pub(crate) fn parse_mcp_environment_handles(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut handles = BTreeMap::new();
    for value in values {
        let (name, handle) = value
            .split_once('=')
            .filter(|(name, handle)| !name.is_empty() && !handle.is_empty())
            .ok_or_else(|| anyhow!("--env values must use NAME=HANDLE"))?;
        if handles.insert(name.into(), handle.into()).is_some() {
            return Err(anyhow!("duplicate MCP environment name {name:?}"));
        }
    }
    Ok(handles)
}

pub(crate) fn parse_mcp_header_handles(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut handles = BTreeMap::new();
    for value in values {
        let (name, handle) = value
            .split_once('=')
            .filter(|(name, handle)| !name.is_empty() && !handle.is_empty())
            .ok_or_else(|| anyhow!("--header values must use NAME=HANDLE"))?;
        if name.parse::<reqwest::header::HeaderName>().is_err()
            || handle.len() > 128
            || !handle
                .bytes()
                .enumerate()
                .all(|(index, byte)| byte.is_ascii_alphanumeric() || (index > 0 && byte == b'_'))
        {
            return Err(anyhow!(
                "--header requires a valid HTTP header name and environment variable handle"
            ));
        }
        if handles.insert(name.into(), handle.into()).is_some() {
            return Err(anyhow!("duplicate MCP HTTP header {name:?}"));
        }
    }
    Ok(handles)
}

fn mcp_extensions(catalog: Vec<ExtensionDescriptor>) -> Vec<ExtensionDescriptor> {
    let mut extensions = catalog
        .into_iter()
        .filter(|extension| extension.kind == ExtensionKind::McpServer)
        .collect::<Vec<_>>();
    extensions.sort_by(|left, right| left.id.cmp(&right.id));
    extensions
}

fn open_login_url(url: &str) -> bool {
    open_external_url(url).is_ok()
}

pub(crate) async fn run_mcp_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .mcp_args
        .first()
        .map(String::as_str)
        .context("mcp action was not parsed")?;
    match action {
        "list" | "status" => {
            if args.mcp_args.len() != 1 {
                return Err(anyhow!("mcp {action} does not accept arguments"));
            }
            let extensions = mcp_extensions(api.extension_catalog().await?);
            if extensions.is_empty() {
                println!("No MCP servers are configured.");
                return Ok(());
            }
            for extension in extensions {
                println!(
                    "{}\t{}\t{}",
                    extension.id,
                    mcp_status_name(&extension.status),
                    extension.source_uri
                );
                if action == "status" {
                    if extension.tool_names.is_empty() {
                        println!("  tools: none discovered");
                    } else {
                        println!("  tools: {}", extension.tool_names.join(", "));
                    }
                    println!(
                        "  authentication: {}",
                        if extension.oauth_supported {
                            if extension.authenticated {
                                "authenticated"
                            } else {
                                "required"
                            }
                        } else {
                            "not supported"
                        }
                    );
                    if let Some(error) = extension.error {
                        println!("  error: {error}");
                    }
                }
            }
        }
        "resources" => {
            if !(2..=3).contains(&args.mcp_args.len()) {
                return Err(anyhow!("usage: opencoding mcp resources <id> [cursor]"));
            }
            let server_id = args.mcp_args[1].trim_start_matches("mcp:");
            let page = api
                .mcp_resources(server_id, args.mcp_args.get(2).map(String::as_str))
                .await?;
            if page.resources.is_empty() {
                println!("No resources exposed by MCP server {server_id}.");
            }
            for resource in page.resources {
                println!(
                    "{}\t{}\t{}",
                    resource.uri,
                    resource.mime_type.as_deref().unwrap_or("unknown"),
                    resource.name,
                );
                if !resource.description.is_empty() {
                    println!("  {}", resource.description);
                }
            }
            if let Some(cursor) = page.next_cursor {
                println!("Next page: opencoding mcp resources {server_id} {cursor}");
            }
        }
        "templates" => {
            if !(2..=3).contains(&args.mcp_args.len()) {
                return Err(anyhow!("usage: opencoding mcp templates <id> [cursor]"));
            }
            let server_id = args.mcp_args[1].trim_start_matches("mcp:");
            let page = api
                .mcp_resource_templates(server_id, args.mcp_args.get(2).map(String::as_str))
                .await?;
            if page.resource_templates.is_empty() {
                println!("No resource templates exposed by MCP server {server_id}.");
            }
            for template in page.resource_templates {
                println!(
                    "{}\t{}\t{}",
                    template.uri_template,
                    template.mime_type.as_deref().unwrap_or("unknown"),
                    template.name,
                );
                if !template.description.is_empty() {
                    println!("  {}", template.description);
                }
            }
            if let Some(cursor) = page.next_cursor {
                println!("Next page: opencoding mcp templates {server_id} {cursor}");
            }
        }
        "read" => {
            if args.mcp_args.len() != 3 {
                return Err(anyhow!("usage: opencoding mcp read <id> <uri>"));
            }
            let server_id = args.mcp_args[1].trim_start_matches("mcp:");
            let resource = api.read_mcp_resource(server_id, &args.mcp_args[2]).await?;
            if resource.contents.is_empty() {
                println!("MCP resource returned no content.");
            }
            for (index, content) in resource.contents.into_iter().enumerate() {
                if index > 0 {
                    println!();
                }
                println!(
                    "{}\t{}",
                    content.uri,
                    content.mime_type.as_deref().unwrap_or("unknown")
                );
                if let Some(text) = content.text {
                    println!("{text}");
                } else if let Some(blob) = content.blob_base64 {
                    println!("[base64 blob · {} characters]", blob.len());
                }
            }
        }
        "add" => {
            if args.mcp_args.len() < 3 {
                return Err(anyhow!(
                    "usage: opencoding mcp add <id> <absolute-program> [arg ...] [--env NAME=HANDLE] [--timeout-ms ms] [--yes]"
                ));
            }
            let server = McpServerSpec {
                id: args.mcp_args[1].clone(),
                program: args.mcp_args[2].clone(),
                args: args.mcp_args[3..].to_vec(),
                environment_handles: parse_mcp_environment_handles(&args.mcp_environment_handles)?,
                timeout_ms: args.mcp_timeout_ms,
            };
            let preview = api.preview_mcp_server(server.clone()).await?;
            println!(
                "MCP server {} will run after the local service restarts.",
                preview.descriptor.name
            );
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change("Install this MCP server?", args.yes)? {
                println!("Installation cancelled.");
                return Ok(());
            }
            let installed = api
                .install_mcp_server(server, preview.permissions_sha256)
                .await?;
            println!(
                "Installed {}. Run `opencoding restart` to connect it.",
                installed.descriptor.id
            );
        }
        "add-http" => {
            if args.mcp_args.len() != 3 {
                return Err(anyhow!(
                    "usage: opencoding mcp add-http <id> <https-url> [--header NAME=HANDLE | --oauth [--oauth-client-id id] [--oauth-scope scope]] [--timeout-ms ms] [--yes]"
                ));
            }
            if args.mcp_oauth && !args.mcp_header_handles.is_empty() {
                return Err(anyhow!(
                    "--oauth cannot be combined with static --header credentials"
                ));
            }
            let server = McpHttpServerSpec {
                id: args.mcp_args[1].clone(),
                endpoint: args.mcp_args[2].clone(),
                header_handles: parse_mcp_header_handles(&args.mcp_header_handles)?,
                oauth: args.mcp_oauth.then(|| McpOAuthSpec {
                    client_id: args.mcp_oauth_client_id.clone(),
                    scopes: args.mcp_oauth_scopes.clone(),
                }),
                timeout_ms: args.mcp_timeout_ms,
            };
            let preview = api.preview_mcp_http_server(server.clone()).await?;
            println!(
                "MCP server {} will connect over Streamable HTTP after the local service restarts.",
                preview.descriptor.name
            );
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change("Install this MCP HTTP server?", args.yes)? {
                println!("Installation cancelled.");
                return Ok(());
            }
            let installed = api
                .install_mcp_http_server(server, preview.permissions_sha256)
                .await?;
            println!(
                "Installed {}. Run `opencoding restart` to connect it.",
                installed.descriptor.id
            );
        }
        "remove" => {
            if args.mcp_args.len() != 2 {
                return Err(anyhow!("usage: opencoding mcp remove <id> [--yes]"));
            }
            let requested = &args.mcp_args[1];
            let extension_id = if requested.starts_with("mcp:") {
                requested.clone()
            } else {
                format!("mcp:{requested}")
            };
            let extension = mcp_extensions(api.extension_catalog().await?)
                .into_iter()
                .find(|extension| extension.id == extension_id)
                .with_context(|| format!("MCP server {extension_id:?} is not configured"))?;
            let local_stdio = extension
                .source_uri
                .starts_with("opencoding://extensions/mcp/");
            let local_http = extension
                .source_uri
                .starts_with("opencoding://extensions/mcp-http/");
            if !local_stdio && !local_http {
                return Err(anyhow!(
                    "{extension_id} is managed by {}; remove it from that source instead",
                    extension.source_uri
                ));
            }
            let permissions_sha256 = extension
                .permissions_sha256
                .clone()
                .context("MCP server cannot be removed without its permission digest")?;
            println!("MCP server {} will be removed.", extension.name);
            print_mcp_permissions(&extension.permissions);
            if !confirm_mcp_change("Remove this MCP server?", args.yes)? {
                println!("Removal cancelled.");
                return Ok(());
            }
            if local_http {
                api.remove_mcp_http_server(&extension.id, permissions_sha256)
                    .await?;
            } else {
                api.remove_mcp_server(&extension.id, permissions_sha256)
                    .await?;
            }
            println!(
                "Removed {}. Run `opencoding restart` to disconnect it.",
                extension.id
            );
        }
        "login" => {
            if args.mcp_args.len() != 2 {
                return Err(anyhow!("usage: opencoding mcp login <id>"));
            }
            let server_id = args.mcp_args[1].trim_start_matches("mcp:");
            let discovery = api.preview_mcp_oauth(server_id).await?;
            println!("MCP server: {}", discovery.server_id);
            println!("Identity provider: {}", discovery.authorization_server);
            println!(
                "Scopes: {}",
                if discovery.scopes.is_empty() {
                    "none requested".into()
                } else {
                    discovery.scopes.join(", ")
                }
            );
            if !confirm_mcp_change("Start this OAuth login?", args.yes)? {
                println!("Login cancelled.");
                return Ok(());
            }
            let launch = api
                .start_mcp_oauth(server_id, discovery.permissions_sha256)
                .await?;
            println!("Open this URL to continue:\n{}", launch.authorization_url);
            if open_login_url(&launch.authorization_url) {
                println!("Opened the system browser. Waiting for completion…");
            } else {
                println!("Could not open a browser automatically. Open the URL above.");
            }
            loop {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                if now >= launch.expires_at.timestamp() {
                    return Err(anyhow!("MCP OAuth login expired; run the command again"));
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        println!("Stopped waiting. The browser login remains valid until it expires.");
                        return Ok(());
                    }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
                if api.mcp_oauth_status(server_id).await?.authenticated {
                    println!("Authenticated {server_id}. Run `opencoding restart` to connect it.");
                    break;
                }
            }
        }
        "logout" => {
            if args.mcp_args.len() != 2 {
                return Err(anyhow!("usage: opencoding mcp logout <id>"));
            }
            let server_id = args.mcp_args[1].trim_start_matches("mcp:");
            let status = api.mcp_oauth_status(server_id).await?;
            if !status.supported {
                return Err(anyhow!("MCP server {server_id:?} does not use OAuth"));
            }
            if !status.authenticated {
                println!("MCP server {server_id} is already logged out.");
                return Ok(());
            }
            if !confirm_mcp_change("Revoke this MCP OAuth login?", args.yes)? {
                println!("Logout cancelled.");
                return Ok(());
            }
            api.logout_mcp_oauth(server_id).await?;
            println!("Logged out {server_id}. Run `opencoding restart` to disconnect it.");
        }
        _ => {
            return Err(anyhow!(
                "unknown mcp action {action:?}; expected list, status, resources, templates, read, add, add-http, login, logout, or remove"
            ));
        }
    }
    Ok(())
}

fn skill_extensions(catalog: Vec<ExtensionDescriptor>) -> Vec<ExtensionDescriptor> {
    let mut extensions = catalog
        .into_iter()
        .filter(|extension| extension.kind == ExtensionKind::Skill)
        .collect::<Vec<_>>();
    extensions.sort_by(|left, right| left.id.cmp(&right.id));
    extensions
}

pub(crate) async fn run_skill_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .skill_args
        .first()
        .map(String::as_str)
        .context("skill action was not parsed")?;
    match action {
        "list" | "status" => {
            if args.skill_args.len() != 1 {
                return Err(anyhow!("skill {action} does not accept arguments"));
            }
            let extensions = skill_extensions(api.extension_catalog().await?);
            if extensions.is_empty() {
                println!("No Skills are configured.");
                return Ok(());
            }
            for extension in extensions {
                println!(
                    "{}\t{}\t{}",
                    extension.id,
                    mcp_status_name(&extension.status),
                    extension.source_uri
                );
                if action == "status" {
                    println!("  {}", extension.description);
                    print_mcp_permissions(&extension.permissions);
                    if let Some(error) = extension.error {
                        println!("  error: {error}");
                    }
                }
            }
        }
        "add" => {
            if args.skill_args.len() != 3 {
                return Err(anyhow!(
                    "usage: opencoding skill add <id> <absolute-SKILL.md> [--name name] --description text [--match term] [--mcp id] [--manual-only] [--yes]"
                ));
            }
            let description = args
                .skill_description
                .clone()
                .context("skill add requires --description")?;
            if !args.skill_manual_only && args.skill_activation_terms.is_empty() {
                return Err(anyhow!(
                    "skill add requires at least one --match term or --manual-only"
                ));
            }
            let path = PathBuf::from(&args.skill_args[2]);
            if !path.is_absolute() {
                return Err(anyhow!("Skill source must be an absolute SKILL.md path"));
            }
            let source_uri = url::Url::from_file_path(&path)
                .map_err(|_| anyhow!("Skill source cannot be represented as file://"))?
                .to_string();
            let id = args.skill_args[1].clone();
            let skill = SkillSpec {
                name: args.skill_name.clone().unwrap_or_else(|| id.clone()),
                id,
                description,
                source_uri,
                activation_terms: args.skill_activation_terms.clone(),
                mcp_dependencies: args.skill_mcp_dependencies.clone(),
                auto_match: !args.skill_manual_only,
            };
            let preview = api.preview_skill(skill.clone()).await?;
            println!(
                "Skill {} will be available immediately after installation.",
                preview.descriptor.name
            );
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change("Install this Skill?", args.yes)? {
                println!("Installation cancelled.");
                return Ok(());
            }
            let installed = api.install_skill(skill, preview.permissions_sha256).await?;
            println!(
                "Installed {}. Invoke it with `${}`.",
                installed.descriptor.id, args.skill_args[1]
            );
        }
        "enable" | "disable" => {
            if args.skill_args.len() != 2 {
                return Err(anyhow!("usage: opencoding skill {action} <id>"));
            }
            let requested = args.skill_args[1].trim_start_matches("skill:");
            let installation = api.skill_installation(requested).await?;
            let enabled = action == "enable";
            api.set_skill_enabled(requested, enabled, installation.revision)
                .await?;
            println!(
                "{} skill:{}.",
                if enabled { "Enabled" } else { "Disabled" },
                requested
            );
        }
        "remove" => {
            if args.skill_args.len() != 2 {
                return Err(anyhow!("usage: opencoding skill remove <id> [--yes]"));
            }
            let requested = args.skill_args[1].trim_start_matches("skill:");
            let extension_id = format!("skill:{requested}");
            let extension = skill_extensions(api.extension_catalog().await?)
                .into_iter()
                .find(|extension| extension.id == extension_id)
                .with_context(|| format!("Skill {extension_id:?} is not configured"))?;
            let permissions_sha256 = extension
                .permissions_sha256
                .clone()
                .context("Skill cannot be removed without its permission digest")?;
            println!("Skill {} will be removed.", extension.name);
            print_mcp_permissions(&extension.permissions);
            if !confirm_mcp_change("Remove this Skill?", args.yes)? {
                println!("Removal cancelled.");
                return Ok(());
            }
            api.remove_skill(requested, permissions_sha256).await?;
            println!("Removed {extension_id}.");
        }
        _ => {
            return Err(anyhow!(
                "unknown skill action {action:?}; expected list, status, add, enable, disable, or remove"
            ));
        }
    }
    Ok(())
}

fn hook_extensions(catalog: Vec<ExtensionDescriptor>) -> Vec<ExtensionDescriptor> {
    let mut extensions = catalog
        .into_iter()
        .filter(|extension| extension.kind == ExtensionKind::Hook)
        .collect::<Vec<_>>();
    extensions.sort_by(|left, right| left.id.cmp(&right.id));
    extensions
}

pub(crate) async fn run_hook_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .hook_args
        .first()
        .map(String::as_str)
        .context("hook action was not parsed")?;
    match action {
        "list" | "status" => {
            if args.hook_args.len() != 1 {
                return Err(anyhow!("hook {action} does not accept arguments"));
            }
            let extensions = hook_extensions(api.extension_catalog().await?);
            if extensions.is_empty() {
                println!("No Hooks are configured.");
                return Ok(());
            }
            for extension in extensions {
                println!(
                    "{}\t{}\t{}",
                    extension.id,
                    mcp_status_name(&extension.status),
                    extension.source_uri
                );
                if action == "status" {
                    print_mcp_permissions(&extension.permissions);
                    if let Some(error) = extension.error {
                        println!("  error: {error}");
                    }
                }
            }
        }
        "add" => {
            if args.hook_args.len() < 4 {
                return Err(anyhow!(
                    "usage: opencoding hook add <id> <pre-tool-use|post-tool-use> <absolute-program> [arg ...] [--name name] [--env NAME=HANDLE] [--timeout-ms ms] [--modify-input] [--yes]"
                ));
            }
            let event = match args.hook_args[2].as_str() {
                "pre-tool-use" => HookEvent::PreToolUse,
                "post-tool-use" => HookEvent::PostToolUse,
                _ => {
                    return Err(anyhow!("Hook event must be pre-tool-use or post-tool-use"));
                }
            };
            if event == HookEvent::PostToolUse && args.hook_modify_input {
                return Err(anyhow!("only a pre-tool-use Hook may use --modify-input"));
            }
            let id = args.hook_args[1].clone();
            let hook = HookSpec {
                name: args.hook_name.clone().unwrap_or_else(|| id.clone()),
                id,
                event,
                program: args.hook_args[3].clone(),
                args: args.hook_args[4..].to_vec(),
                environment_handles: parse_mcp_environment_handles(&args.mcp_environment_handles)?,
                timeout_ms: args.mcp_timeout_ms.min(10_000),
                can_modify_input: args.hook_modify_input,
            };
            let preview = api.preview_hook(hook.clone()).await?;
            println!(
                "Hook {} will run immediately after installation.",
                preview.descriptor.name
            );
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change("Install this Hook?", args.yes)? {
                println!("Installation cancelled.");
                return Ok(());
            }
            let installed = api.install_hook(hook, preview.permissions_sha256).await?;
            println!("Installed {}.", installed.descriptor.id);
        }
        "remove" => {
            if args.hook_args.len() != 2 {
                return Err(anyhow!("usage: opencoding hook remove <id> [--yes]"));
            }
            let requested = args.hook_args[1].trim_start_matches("hook:");
            let extension_id = format!("hook:{requested}");
            let extension = hook_extensions(api.extension_catalog().await?)
                .into_iter()
                .find(|extension| extension.id == extension_id)
                .with_context(|| format!("Hook {extension_id:?} is not configured"))?;
            let permissions_sha256 = extension
                .permissions_sha256
                .clone()
                .context("Hook cannot be removed without its permission digest")?;
            println!("Hook {} will be removed.", extension.name);
            print_mcp_permissions(&extension.permissions);
            if !confirm_mcp_change("Remove this Hook?", args.yes)? {
                println!("Removal cancelled.");
                return Ok(());
            }
            api.remove_hook(requested, permissions_sha256).await?;
            println!("Removed {extension_id}.");
        }
        _ => {
            return Err(anyhow!(
                "unknown hook action {action:?}; expected list, status, add, or remove"
            ));
        }
    }
    Ok(())
}

pub(crate) fn parse_plugin_selector(selector: &str) -> Result<(&str, &str)> {
    let (plugin, marketplace) = selector
        .split_once('@')
        .context("Plugin selector must be <plugin>@<marketplace>")?;
    if plugin.is_empty()
        || marketplace.is_empty()
        || marketplace.contains('@')
        || !plugin
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !marketplace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(anyhow!(
            "Plugin selector must contain safe <plugin>@<marketplace> segments"
        ));
    }
    Ok((plugin, marketplace))
}

async fn run_plugin_marketplace_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .plugin_args
        .get(1)
        .map(String::as_str)
        .context("plugin marketplace requires list, add, upgrade, or remove")?;
    match action {
        "list" => {
            if args.plugin_args.len() != 2 {
                return Err(anyhow!("plugin marketplace list does not accept arguments"));
            }
            let marketplaces = api.marketplaces().await?;
            if marketplaces.is_empty() {
                println!("No Plugin Marketplaces are configured.");
            }
            for marketplace in marketplaces {
                println!(
                    "{}\trevision {}\t{}\t{}",
                    marketplace.source.name,
                    marketplace.revision,
                    marketplace.manifest_sha256,
                    marketplace.source.source_uri
                );
            }
        }
        "add" => {
            if args.plugin_args.len() != 4 {
                return Err(anyhow!(
                    "usage: opencoding plugin marketplace add <name> <absolute-directory-or-marketplace.json> [--yes]"
                ));
            }
            let path = PathBuf::from(&args.plugin_args[3]);
            if !path.is_absolute() {
                return Err(anyhow!("Marketplace source must be an absolute path"));
            }
            let source = MarketplaceSource {
                name: args.plugin_args[2].clone(),
                source_uri: url::Url::from_file_path(path)
                    .map_err(|_| anyhow!("Marketplace source cannot be represented as file://"))?
                    .to_string(),
                source_kind: MarketplaceSourceKind::Local,
            };
            let preview = api.preview_marketplace(source.clone()).await?;
            println!("Marketplace {} will be indexed.", preview.descriptor.name);
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change("Add this Plugin Marketplace?", args.yes)? {
                println!("Marketplace add cancelled.");
                return Ok(());
            }
            let installed = api
                .add_marketplace(source, preview.permissions_sha256)
                .await?;
            println!(
                "Added Marketplace {} at revision {}.",
                installed.source.name, installed.revision
            );
        }
        "upgrade" | "remove" => {
            if args.plugin_args.len() != 3 {
                return Err(anyhow!(
                    "usage: opencoding plugin marketplace {action} <name> [--yes]"
                ));
            }
            let name = &args.plugin_args[2];
            let preview = api.preview_marketplace_upgrade(name).await?;
            print_mcp_permissions(&preview.descriptor.permissions);
            let prompt = if action == "upgrade" {
                "Refresh this Marketplace snapshot?"
            } else {
                "Remove this Marketplace? Installed Plugins must be removed first."
            };
            if !confirm_mcp_change(prompt, args.yes)? {
                println!("Marketplace {action} cancelled.");
                return Ok(());
            }
            if action == "upgrade" {
                let upgraded = api
                    .upgrade_marketplace(name, preview.permissions_sha256)
                    .await?;
                println!(
                    "Marketplace {} is at revision {}.",
                    upgraded.source.name, upgraded.revision
                );
            } else {
                api.remove_marketplace(name, preview.permissions_sha256)
                    .await?;
                println!("Removed Marketplace {name}.");
            }
        }
        _ => {
            return Err(anyhow!(
                "unknown plugin marketplace action {action:?}; expected list, add, upgrade, or remove"
            ));
        }
    }
    Ok(())
}

pub(crate) async fn run_plugin_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .plugin_args
        .first()
        .map(String::as_str)
        .context("plugin action was not parsed")?;
    if action == "marketplace" {
        return run_plugin_marketplace_command(api, args).await;
    }
    match action {
        "list" => {
            if args.plugin_args.len() != 1 {
                return Err(anyhow!("plugin list does not accept arguments"));
            }
            let plugins = api.plugins().await?;
            if plugins.is_empty() {
                println!("No Marketplace Plugins are available.");
            }
            for plugin in plugins {
                let state = match plugin.descriptor.status {
                    ExtensionStatus::Installed => "installed",
                    ExtensionStatus::Disabled => "disabled",
                    _ => "available",
                };
                println!(
                    "{}\t{}\t{}\t{}",
                    plugin.descriptor.id.trim_start_matches("plugin:"),
                    state,
                    plugin.available_version,
                    plugin.descriptor.source_uri
                );
                if plugin.update_available {
                    println!(
                        "  update: {} → {}",
                        plugin.installed_version.as_deref().unwrap_or("unknown"),
                        plugin.available_version
                    );
                }
            }
        }
        "add" | "update" => {
            if args.plugin_args.len() != 2 {
                return Err(anyhow!(
                    "usage: opencoding plugin {action} <plugin>@<marketplace> [--yes]"
                ));
            }
            let (plugin, marketplace) = parse_plugin_selector(&args.plugin_args[1])?;
            let preview = api
                .preview_plugin(marketplace.into(), plugin.into())
                .await?;
            println!(
                "Plugin {} {}.",
                preview.descriptor.name,
                if preview.requires_restart {
                    "will activate after the local service restarts"
                } else {
                    "will activate immediately"
                }
            );
            print_mcp_permissions(&preview.descriptor.permissions);
            if !confirm_mcp_change(
                if action == "add" {
                    "Install this Plugin?"
                } else {
                    "Update this Plugin to the reviewed Marketplace target?"
                },
                args.yes,
            )? {
                println!("Plugin {action} cancelled.");
                return Ok(());
            }
            let installed = api
                .install_plugin(
                    marketplace.into(),
                    plugin.into(),
                    preview.permissions_sha256,
                )
                .await?;
            println!(
                "Installed {} version {}.",
                installed.descriptor.id, installed.available_version
            );
        }
        "enable" | "disable" => {
            if args.plugin_args.len() != 2 {
                return Err(anyhow!(
                    "usage: opencoding plugin {action} <plugin>@<marketplace>"
                ));
            }
            parse_plugin_selector(&args.plugin_args[1])?;
            let detail = api.plugin(&args.plugin_args[1]).await?;
            let revision = detail
                .summary
                .installed_revision
                .context("Plugin is not installed")?;
            let enabled = action == "enable";
            api.set_plugin_enabled(&args.plugin_args[1], enabled, revision)
                .await?;
            println!(
                "{} plugin:{}.",
                if enabled { "Enabled" } else { "Disabled" },
                args.plugin_args[1]
            );
        }
        "remove" => {
            if args.plugin_args.len() != 2 {
                return Err(anyhow!(
                    "usage: opencoding plugin remove <plugin>@<marketplace> [--yes]"
                ));
            }
            parse_plugin_selector(&args.plugin_args[1])?;
            let detail = api.plugin(&args.plugin_args[1]).await?;
            let permissions_sha256 = detail
                .summary
                .descriptor
                .permissions_sha256
                .clone()
                .context("Plugin cannot be removed without its permission digest")?;
            print_mcp_permissions(&detail.summary.descriptor.permissions);
            if !confirm_mcp_change("Remove this Plugin?", args.yes)? {
                println!("Plugin removal cancelled.");
                return Ok(());
            }
            api.remove_plugin(&args.plugin_args[1], permissions_sha256)
                .await?;
            println!("Removed plugin:{}.", args.plugin_args[1]);
        }
        "read" | "checkout" | "share" => {
            if args.plugin_args.len() != 2 {
                return Err(anyhow!(
                    "usage: opencoding plugin {action} <plugin>@<marketplace>"
                ));
            }
            parse_plugin_selector(&args.plugin_args[1])?;
            let detail = api.plugin(&args.plugin_args[1]).await?;
            if action == "checkout" {
                let uri = url::Url::parse(&detail.summary.descriptor.source_uri)?;
                let path = uri
                    .to_file_path()
                    .map_err(|_| anyhow!("Plugin source is not a local file"))?;
                println!("{}", path.display());
            } else if action == "share" {
                println!(
                    "{}\t{}\t{}",
                    args.plugin_args[1],
                    detail.summary.available_version,
                    detail.summary.descriptor.source_uri
                );
            } else {
                println!(
                    "{}\t{}\t{}",
                    detail.summary.descriptor.id,
                    detail.summary.available_version,
                    detail.summary.descriptor.description
                );
                println!("Marketplace: {}", detail.summary.marketplace_name);
                println!("Source: {}", detail.summary.descriptor.source_uri);
                for component in detail.summary.components {
                    println!(
                        "  {:?}\t{}\t{}",
                        component.kind, component.id, component.name
                    );
                }
            }
        }
        _ => {
            return Err(anyhow!(
                "unknown plugin action {action:?}; expected list, add, update, enable, disable, remove, read, checkout, share, or marketplace"
            ));
        }
    }
    Ok(())
}

pub(crate) async fn run_app_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .app_args
        .first()
        .map(String::as_str)
        .context("app action was not parsed")?;
    match action {
        "list" => {
            if args.app_args.len() != 1 {
                return Err(anyhow!("app list does not accept arguments"));
            }
            let apps = api.apps().await?;
            if apps.is_empty() {
                println!("No Plugin Apps are available.");
            }
            for app in apps {
                println!(
                    "{}\t{}\t{}\t{}",
                    app.app.id,
                    if app.enabled {
                        "installed, enabled"
                    } else if app.installed {
                        "installed, disabled"
                    } else {
                        "available"
                    },
                    app.plugin_id,
                    app.app.name
                );
            }
        }
        "read" => {
            if args.app_args.len() != 2 {
                return Err(anyhow!("usage: opencoding app read <id>"));
            }
            let app = api.app(&args.app_args[1]).await?;
            println!("{}\t{}", app.app.name, app.app.description);
            println!("Plugin: {}", app.plugin_id);
            println!(
                "State: {}",
                if app.enabled {
                    "installed, enabled"
                } else if app.installed {
                    "installed, disabled"
                } else {
                    "available"
                }
            );
            if !app.app.mcp_server_ids.is_empty() {
                println!("MCP servers: {}", app.app.mcp_server_ids.join(", "));
            }
            if !app.app.tool_names.is_empty() {
                println!("Tools: {}", app.app.tool_names.join(", "));
            }
            if let Some(homepage) = app.app.homepage_url {
                println!("Homepage: {homepage}");
            }
        }
        _ => {
            return Err(anyhow!(
                "unknown app action {action:?}; expected list or read"
            ));
        }
    }
    Ok(())
}

fn print_client_presence(client: &ClientPresence) {
    println!(
        "{}\t{:?}\t{}\t{}\t{}",
        client.client_id,
        client.client_kind,
        client.actor_id.0,
        client
            .session_id
            .as_ref()
            .map(|id| id.0.as_str())
            .unwrap_or("no-session"),
        if client.remote { "remote" } else { "local" },
    );
    if let Some(device_id) = &client.device_id {
        println!("  device: {device_id}");
    }
    println!(
        "  state: {} · expires {}",
        if client.focused {
            "focused"
        } else {
            "background"
        },
        client.expires_at
    );
}

pub(crate) async fn run_client_command(api: &Api, args: &CliArgs) -> Result<()> {
    let action = args
        .client_args
        .first()
        .map(String::as_str)
        .context("client action was not parsed")?;
    match action {
        "list" => {
            if args.client_args.len() != 1 {
                return Err(anyhow!("client list does not accept arguments"));
            }
            let clients = api.client_presence().await?;
            if clients.is_empty() {
                println!("No active clients.");
            } else {
                for client in clients {
                    print_client_presence(&client);
                }
            }
        }
        "revoke" => {
            if args.client_args.len() != 2 {
                return Err(anyhow!(
                    "usage: opencoding client revoke <client-id> [--yes]"
                ));
            }
            let client_id = &args.client_args[1];
            let client = api
                .client_presence()
                .await?
                .into_iter()
                .find(|client| &client.client_id == client_id)
                .with_context(|| format!("active client {client_id:?} was not found"))?;
            if !client.remote || !client.revocable {
                return Err(anyhow!(
                    "client {client_id:?} has no revocable remote Team Grant"
                ));
            }
            print_client_presence(&client);
            if !confirm_mcp_change("Revoke this remote client grant?", args.yes)? {
                println!("Revocation cancelled.");
                return Ok(());
            }
            api.remove_client_presence(client_id, true).await?;
            println!("Revoked remote client {client_id}.");
        }
        _ => {
            return Err(anyhow!(
                "unknown client action {action:?}; expected list or revoke"
            ));
        }
    }
    Ok(())
}
