use crate::args::CliArgs;
use anyhow::{Context, Result, anyhow};
use s_code_config::{Component, ConfigLoader, default_user_config_path};
use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct ProviderPreset {
    provider: &'static str,
    base_url: Option<&'static str>,
    credential_handle: Option<&'static str>,
}

fn provider_preset(name: &str) -> Result<ProviderPreset> {
    match name {
        "openrouter" => Ok(ProviderPreset {
            provider: "openai_compatible",
            base_url: Some("https://openrouter.ai/api/v1"),
            credential_handle: Some("OPENROUTER_API_KEY"),
        }),
        "openai" => Ok(ProviderPreset {
            provider: "openai_compatible",
            base_url: Some("https://api.openai.com/v1"),
            credential_handle: Some("OPENAI_API_KEY"),
        }),
        "anthropic" => Ok(ProviderPreset {
            provider: "anthropic",
            base_url: Some("https://api.anthropic.com/v1"),
            credential_handle: Some("ANTHROPIC_API_KEY"),
        }),
        "gemini" => Ok(ProviderPreset {
            provider: "gemini",
            base_url: Some("https://generativelanguage.googleapis.com/v1beta"),
            credential_handle: Some("GEMINI_API_KEY"),
        }),
        "local" => Ok(ProviderPreset {
            provider: "openai_compatible",
            base_url: Some("http://127.0.0.1:11434/v1"),
            credential_handle: None,
        }),
        "openai-compatible" => Ok(ProviderPreset {
            provider: "openai_compatible",
            base_url: None,
            credential_handle: None,
        }),
        _ => Err(anyhow!(
            "provider must be openrouter, openai, anthropic, gemini, local, or openai-compatible"
        )),
    }
}

fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    print!(
        "{label}{}: ",
        default
            .map(|value| format!(" [{value}]"))
            .unwrap_or_default()
    );
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        default
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("{label} is required"))
    } else {
        Ok(value.to_owned())
    }
}

fn prompt_provider() -> Result<String> {
    println!("Choose a model endpoint:");
    println!("  1. OpenRouter");
    println!("  2. OpenAI");
    println!("  3. Anthropic");
    println!("  4. Gemini");
    println!("  5. Local OpenAI-compatible endpoint");
    println!("  6. Custom OpenAI-compatible endpoint");
    let selected = prompt("Selection", Some("1"))?;
    match selected.as_str() {
        "1" => Ok("openrouter".into()),
        "2" => Ok("openai".into()),
        "3" => Ok("anthropic".into()),
        "4" => Ok("gemini".into()),
        "5" => Ok("local".into()),
        "6" => Ok("openai-compatible".into()),
        _ => provider_preset(&selected).map(|_| selected),
    }
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
}

fn valid_credential_handle(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_uppercase() || (index > 0 && byte.is_ascii_digit())
        })
}

fn validate_model_base_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("model API base URL is invalid")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow!("model API base URL must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(anyhow!(
            "model API base URL must not contain a query or fragment"
        ));
    }
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(anyhow!(
            "model API base URL must use HTTPS or loopback HTTP"
        ));
    }
    Ok(())
}

fn config_path() -> Result<PathBuf> {
    match env::var_os("S_CODE_CONFIG") {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        Some(_) => Err(anyhow!("S_CODE_CONFIG must not be empty")),
        None => default_user_config_path().map_err(anyhow::Error::msg),
    }
}

fn existing_document(path: &Path) -> Result<toml::Value> {
    if !path.exists() {
        return Ok(toml::Value::Table(toml::Table::new()));
    }
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("cannot inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(anyhow!("configuration must be a regular file"));
    }
    if metadata.len() > 1024 * 1024 {
        return Err(anyhow!("configuration exceeds 1 MiB"));
    }
    let source =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    toml::from_str(&source).context("existing configuration is invalid TOML")
}

fn table(value: &mut toml::Value) -> Result<&mut toml::Table> {
    value
        .as_table_mut()
        .ok_or_else(|| anyhow!("configuration root must be a table"))
}

fn update_document(
    mut document: toml::Value,
    provider: &str,
    base_url: &str,
    credential_handle: Option<&str>,
    model: &str,
) -> Result<String> {
    let root = table(&mut document)?;
    root.insert("schema_version".into(), toml::Value::Integer(1));
    root.entry("profile")
        .or_insert_with(|| toml::Value::String("development".into()));

    let client = root
        .entry("client")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    table(client)?.insert("model".into(), toml::Value::String(model.into()));

    let model_config = root
        .entry("model")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let model_config = table(model_config)?;
    model_config.remove("endpoints");
    model_config.insert("provider".into(), toml::Value::String(provider.into()));
    model_config.insert("base_url".into(), toml::Value::String(base_url.into()));
    match credential_handle {
        Some(handle) => {
            model_config.insert(
                "credential_handle".into(),
                toml::Value::String(handle.into()),
            );
        }
        None => {
            model_config.remove("credential_handle");
        }
    }
    toml::to_string_pretty(&document).context("cannot encode configuration")
}

fn write_private_configuration(path: &Path, contents: &str) -> Result<()> {
    let directory = path.parent().context("configuration path has no parent")?;
    create_private_directories(directory)?;
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("cannot inspect {}", directory.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(anyhow!(
            "configuration directory must be a regular directory"
        ));
    }
    let temporary = directory.join(format!(
        ".config.toml.{}.{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        ConfigLoader::new()
            .with_file(&temporary)
            .with_environment(env::vars_os())
            .load(Component::Daemon)
            .context("generated daemon configuration is invalid")?;
        ConfigLoader::new()
            .with_file(&temporary)
            .with_environment(env::vars_os())
            .load(Component::Cli)
            .context("generated CLI configuration is invalid")?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_private_directories(directory: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut cursor = directory;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor
            .parent()
            .context("configuration directory has no existing ancestor")?;
    }
    for path in missing.into_iter().rev() {
        match fs::create_dir(&path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot create {}", path.display()));
            }
        }
    }
    Ok(())
}

pub(crate) fn run_setup(args: &CliArgs) -> Result<()> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let provider_name = match args.setup_provider.clone() {
        Some(provider) => provider,
        None if interactive => prompt_provider()?,
        None => return Err(anyhow!("non-interactive setup requires --provider")),
    };
    let preset = provider_preset(&provider_name)?;
    let base_url = match args
        .setup_base_url
        .clone()
        .or_else(|| preset.base_url.map(str::to_owned))
    {
        Some(url) => url,
        None if interactive => prompt("Model API base URL", None)?,
        None => return Err(anyhow!("setup requires --base-url for this provider")),
    };
    validate_model_base_url(&base_url)?;
    let model = match args.model.clone() {
        Some(model) => model,
        None if interactive => prompt("Model ID", None)?,
        None => return Err(anyhow!("non-interactive setup requires --model")),
    };
    if !valid_model_id(&model) {
        return Err(anyhow!(
            "model ID must be non-empty, contain no whitespace, and be at most 256 bytes"
        ));
    }
    let credential_handle = args
        .setup_credential_handle
        .as_deref()
        .or(preset.credential_handle)
        .map(str::to_owned);
    if let Some(handle) = credential_handle.as_deref()
        && !valid_credential_handle(handle)
    {
        return Err(anyhow!(
            "credential handle must be an uppercase environment variable name"
        ));
    }
    let path = config_path()?;
    if !path.is_absolute() {
        return Err(anyhow!("configuration path must be absolute"));
    }
    if path.exists() && !args.yes {
        if !interactive {
            return Err(anyhow!(
                "configuration already exists at {}; rerun with --yes to update its model settings",
                path.display()
            ));
        }
        let answer = prompt("Update the existing model settings? (y/N)", Some("N"))?;
        if !matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Setup cancelled; existing configuration was preserved.");
            return Ok(());
        }
    }
    let contents = update_document(
        existing_document(&path)?,
        preset.provider,
        &base_url,
        credential_handle.as_deref(),
        &model,
    )?;
    write_private_configuration(&path, &contents)?;
    println!("✓ configuration saved to {}", path.display());
    println!("✓ model {model} via {base_url}");
    if let Some(handle) = credential_handle {
        if env::var_os(&handle).is_some_and(|value| !value.is_empty()) {
            println!("✓ credential handle {handle} is available in this environment");
        } else {
            println!("! set {handle} in your shell or secret manager before starting S-Code");
        }
    } else {
        println!("✓ endpoint requires no provider credential");
    }
    println!("Next: run `s-code doctor`, then `s-code`.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_updates_only_model_settings() {
        let existing: toml::Value = toml::from_str(
            "schema_version = 1\n[client]\ntheme = 'light'\n[daemon]\nlisten = '127.0.0.1:0'\n",
        )
        .unwrap();
        let updated = update_document(
            existing,
            "anthropic",
            "https://api.anthropic.com/v1",
            Some("ANTHROPIC_API_KEY"),
            "claude-model",
        )
        .unwrap();
        let value: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(value["client"]["theme"].as_str(), Some("light"));
        assert_eq!(value["client"]["model"].as_str(), Some("claude-model"));
        assert_eq!(value["model"]["provider"].as_str(), Some("anthropic"));
        assert_eq!(
            value["model"]["credential_handle"].as_str(),
            Some("ANTHROPIC_API_KEY")
        );
    }

    #[test]
    fn setup_rejects_unknown_providers_and_unsafe_model_ids() {
        assert!(provider_preset("unknown").is_err());
        assert!(!valid_model_id("two words"));
        assert!(!valid_model_id(""));
        assert!(valid_model_id("provider/model"));
        assert!(validate_model_base_url("https://models.example/v1").is_ok());
        assert!(validate_model_base_url("http://127.0.0.1:11434/v1").is_ok());
        assert!(validate_model_base_url("http://models.example/v1").is_err());
        assert!(valid_credential_handle("MODEL_API_KEY_2"));
        assert!(!valid_credential_handle("raw-secret"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_does_not_change_existing_parent_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let shared = directory.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        let path = shared.join("nested").join("config.toml");
        write_private_configuration(&path, "schema_version = 1\n").unwrap();
        assert_eq!(
            fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(shared.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
