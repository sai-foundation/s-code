//! `s-code-skill-registry`: serve the online shared skill registry, or manage
//! its principals from the same host. Tokens are printed exactly once at
//! creation and are never stored, logged or shown again.
use s_code_skill_registry::{CONNECTION_FILE, RegistryConfig, RegistryStore, start_with};
use std::{collections::BTreeMap, sync::Arc};

const USAGE: &str = "usage:
  s-code-skill-registry serve
  s-code-skill-registry principal create --display-name NAME --organization ORG --team TEAM [--authorized-evaluator true]
  s-code-skill-registry principal authorize-evaluator --id PRINCIPAL_ID
  s-code-skill-registry principal revoke-evaluator --id PRINCIPAL_ID
  s-code-skill-registry principal disable --id PRINCIPAL_ID
  s-code-skill-registry principal list

environment:
  S_CODE_SKILL_REGISTRY_BIND               listen address (default 127.0.0.1:18790)
  S_CODE_SKILL_REGISTRY_DATA_DIR           SQLite data directory (default ./skill-registry-data)
  S_CODE_SKILL_REGISTRY_BEHIND_TLS_PROXY   set to 1 only when a TLS-terminating reverse proxy fronts a non-loopback bind
  S_CODE_SKILL_REGISTRY_INSECURE_COOKIES   loopback development only: shop session cookies without Secure (refused off loopback)
  RUST_LOG                                 tracing filter (default info for this service)";

fn options(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut parsed = BTreeMap::new();
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        let Some(name) = flag.strip_prefix("--") else {
            return Err(format!("unexpected argument {flag:?}\n{USAGE}"));
        };
        let value = iter
            .next()
            .ok_or_else(|| format!("--{name} needs a value\n{USAGE}"))?;
        parsed.insert(name.to_owned(), value.clone());
    }
    Ok(parsed)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required\n{USAGE}"))
}

async fn serve(config: RegistryConfig) -> anyhow::Result<()> {
    let store = Arc::new(RegistryStore::open(&config.data_dir).await?);
    let running = start_with(config.bind, store, config.web_settings()).await?;
    let connection = serde_json::json!({
        "url": running.url(),
        "pid": std::process::id(),
        "started_at": chrono::Utc::now(),
    });
    let connection_path = config.data_dir.join(CONNECTION_FILE);
    std::fs::write(&connection_path, connection.to_string())?;
    eprintln!("S_CODE_SKILL_REGISTRY_ADDR={}", running.address);
    tracing::info!(
        address = %running.address,
        data_dir = %config.data_dir.display(),
        "S-Code skill registry listening"
    );
    shutdown_signal().await;
    tracing::info!("shutdown requested; draining in-flight requests");
    running.stop().await;
    let _ = std::fs::remove_file(&connection_path);
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler must be installable");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn principal(config: RegistryConfig, args: &[String]) -> anyhow::Result<()> {
    let store = RegistryStore::open(&config.data_dir).await?;
    let (action, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("{USAGE}"))?;
    let options = options(rest).map_err(|message| anyhow::anyhow!(message))?;
    match action.as_str() {
        "create" => {
            let display_name =
                required(&options, "display-name").map_err(|m| anyhow::anyhow!(m))?;
            let organization =
                required(&options, "organization").map_err(|m| anyhow::anyhow!(m))?;
            let team = required(&options, "team").map_err(|m| anyhow::anyhow!(m))?;
            let authorized_evaluator = match options.get("authorized-evaluator").map(String::as_str)
            {
                None | Some("false") => false,
                Some("true") => true,
                Some(other) => {
                    anyhow::bail!("--authorized-evaluator must be true or false, not {other:?}")
                }
            };
            let (principal, token) = store
                .create_principal_with(display_name, organization, team, authorized_evaluator)
                .await?;
            // The only time the token is ever shown. It is not written to the
            // database, the log or the connection file.
            println!(
                "{}",
                serde_json::json!({
                    "principal": principal,
                    "token": token,
                    "note": "store this token in a secret manager now; it cannot be shown again",
                })
            );
        }
        "disable" => {
            let id = required(&options, "id").map_err(|m| anyhow::anyhow!(m))?;
            let principal = store.disable_principal(id).await?;
            println!("{}", serde_json::to_string(&principal)?);
        }
        "authorize-evaluator" | "revoke-evaluator" => {
            let id = required(&options, "id").map_err(|m| anyhow::anyhow!(m))?;
            let principal = store
                .set_authorized_evaluator(id, action == "authorize-evaluator")
                .await?;
            println!("{}", serde_json::to_string(&principal)?);
        }
        "list" => {
            let principals = store.list_principals().await?;
            println!("{}", serde_json::to_string(&principals)?);
        }
        other => anyhow::bail!("unknown principal action {other:?}\n{USAGE}"),
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "s_code_skill_registry=info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = RegistryConfig::from_env()?;
    match args.first().map(String::as_str) {
        Some("serve") if args.len() == 1 => serve(config).await,
        Some("principal") => principal(config, &args[1..]).await,
        Some("--help") | Some("-h") | Some("help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => anyhow::bail!("{USAGE}"),
    }
}
