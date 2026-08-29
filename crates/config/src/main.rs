use opencoding_config::{Component, ConfigLoader, reload_behavior};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let command = args.first().map(String::as_str).unwrap_or("help");
    if command == "help" || command == "--help" || command == "-h" {
        print_help();
        return Ok(());
    }
    let component = option(&args, "--component").ok_or("--component is required")?;
    let component = match component.as_str() {
        "daemon" => Component::Daemon,
        "control-plane" => Component::ControlPlane,
        "runner" => Component::Runner,
        "cli" | "client" => Component::Cli,
        _ => return Err(format!("unknown component: {component}").into()),
    };
    let mut loader = ConfigLoader::from_process();
    if let Some(path) = option(&args, "--config") {
        loader = loader.with_file(PathBuf::from(path));
    }
    let effective = loader.load(component)?;
    match command {
        "validate" => println!(
            "configuration is valid (schema v{})",
            effective.config.schema_version
        ),
        "print-effective" => println!(
            "{}",
            serde_json::to_string_pretty(&effective.redacted_json())?
        ),
        "explain" => {
            let field = positional_after_command(&args).ok_or("explain requires a field path")?;
            let source = effective
                .provenance(field)
                .ok_or("unknown configuration field")?;
            println!(
                "{field}: {:?} ({}) · reload={:?}",
                source.source,
                source.detail,
                reload_behavior(field)
            );
        }
        _ => return Err(format!("unknown command: {command}").into()),
    }
    Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn positional_after_command(args: &[String]) -> Option<&str> {
    args.iter()
        .skip(1)
        .find(|value| {
            !value.starts_with('-')
                && !matches!(
                    value.as_str(),
                    "daemon" | "control-plane" | "runner" | "cli" | "client"
                )
        })
        .map(String::as_str)
}

fn print_help() {
    println!(
        "opencoding-config <validate|print-effective|explain FIELD> --component <daemon|control-plane|runner|cli> [--config PATH]"
    );
}
