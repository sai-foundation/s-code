use crate::{api::Api, args::CliArgs};
use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};

const USAGE: &str = "Usage: s-code im telegram connect [--credential-handle TELEGRAM_BOT_TOKEN]\n       s-code im telegram status|pair|sessions|revoke|disconnect\n       s-code im telegram approve <telegram-user-id>\n       s-code im telegram allow|disallow <session-id>";

pub(crate) async fn run(api: &Api, args: &CliArgs) -> Result<()> {
    let values = &args.im_args;
    if values.first().map(String::as_str) != Some("telegram") {
        return Err(anyhow!(USAGE));
    }
    let action = values.get(1).map(String::as_str).context(USAGE)?;
    let extra = values.get(2);
    let expects_extra = matches!(action, "approve" | "allow" | "disallow");
    if values.len() != if expects_extra { 3 } else { 2 } {
        return Err(anyhow!(USAGE));
    }
    if args.setup_credential_handle.is_some() && action != "connect" {
        return Err(anyhow!("--credential-handle is only used with connect"));
    }
    if action == "sessions" {
        for session in api.sessions().await? {
            println!("{}  {}", session.id.0, session.title);
        }
        println!("Authorize a session: s-code im telegram allow <session-id>");
        return Ok(());
    }
    let result: Value = if action == "status" {
        api.json(api.request(Method::GET, "/v1/im/telegram").query(&[
            ("organization_id", &api.scope.organization_id.0),
            ("team_id", &api.scope.team_id.0),
            ("actor_id", &api.scope.actor_id.0),
        ]))
        .await?
    } else {
        let mut body = json!({"scope":api.scope,"action":action});
        match action {
            "connect" => {
                println!(
                    "Create a bot with @BotFather in Telegram, then enter its token.\nYour bot token is stored in S-Code's encrypted local database."
                );
                let token = if let Some(handle) = &args.setup_credential_handle {
                    std::env::var(handle)
                        .context("The requested token environment variable is not set")?
                } else {
                    read_token()?
                };
                body["token"] = json!(token);
            }
            "approve" => {
                body["user_id"] = json!(
                    extra
                        .unwrap()
                        .parse::<i64>()
                        .context("Telegram user ID must be numeric")?
                )
            }
            "allow" | "disallow" => body["session_id"] = json!(extra.unwrap()),
            "pair" | "revoke" | "disconnect" => {}
            _ => return Err(anyhow!(USAGE)),
        }
        api.json(api.request(Method::POST, "/v1/im/telegram").json(&body))
            .await?
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    if let Some(link) = result["pairing_link"].as_str() {
        println!(
            "\nOpen this private link on your phone (expires in 10 minutes):\n{link}\n\nThen run: s-code im telegram status\nVerify your Telegram user ID, then: s-code im telegram approve <user-id>"
        );
    }
    if matches!(action, "revoke" | "disconnect" | "disallow") {
        println!(
            "Remote access removed. Already started tasks remain visible in S-Code; stop them there if needed."
        );
    }
    Ok(())
}
fn read_token() -> Result<String> {
    if !io::stdin().is_terminal() {
        return Err(anyhow!(
            "Use --credential-handle TELEGRAM_BOT_TOKEN for non-interactive setup"
        ));
    }
    print!("Bot token (hidden): ");
    io::stdout().flush()?;
    terminal::enable_raw_mode()?;
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
        }
    }
    let _restore = Restore;
    let mut value = String::new();
    loop {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Enter => {
                    println!("\r");
                    break;
                }
                KeyCode::Esc => return Err(anyhow!("Connection cancelled")),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(anyhow!("Connection cancelled"));
                }
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Char(c) if !c.is_control() && value.len() < 256 => value.push(c),
                _ => {}
            }
        }
    }
    if value.trim().is_empty() {
        return Err(anyhow!("Bot token is required"));
    }
    Ok(value.trim().to_owned())
}
