//! Private, per-installation provider credentials. Never included in diagnostics.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedProvider {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

pub fn path() -> Result<PathBuf, String> {
    Ok(std::env::var_os("S_CODE_CONFIG")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(super::default_user_config_path)?
        .with_extension("provider-credentials.json"))
}

pub fn read(path: &Path) -> Result<Option<SavedProvider>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot read saved provider credentials".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 32768 {
        return Err("Saved provider credentials must be a small regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Saved provider credentials must have permissions 0600".into());
        }
    }
    let file = fs::File::open(path).map_err(|_| "Cannot read saved provider credentials")?;
    let opened = file
        .metadata()
        .map_err(|_| "Cannot inspect saved provider credentials")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.ino() != metadata.ino() || opened.dev() != metadata.dev() {
            return Err("Provider credentials changed while opening the file; retry".into());
        }
    }
    if !opened.is_file() {
        return Err("Provider credentials must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read saved provider credentials")?;
    if bytes.len() > 32768 {
        return Err("Saved provider credentials are too large".into());
    }

    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Invalid saved provider credentials".into())
}

pub fn save(path: &Path, provider: &SavedProvider) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Credential path must be absolute".into());
    }
    let parent = path.parent().ok_or("Credential path has no parent")?;
    // Do not alter existing parent directory permissions.
    let mut missing = Vec::new();
    let mut cursor = parent;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or("Invalid credential directory")?;
    }
    for directory in missing.iter().rev() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .map_err(|_| "Cannot create credential directory")?;
        }
        #[cfg(not(unix))]
        fs::create_dir(directory).map_err(|_| "Cannot create credential directory")?;
    }
    if fs::symlink_metadata(parent)
        .map_err(|_| "Cannot inspect credential directory")?
        .file_type()
        .is_symlink()
    {
        return Err("Credential directory must not be a symbolic link".into());
    }
    // Refuse unsafe existing targets; never follow a symlink during replacement.
    read(path)?;
    let mut file = tempfile::Builder::new()
        .prefix(".provider-")
        .suffix(".provider-credentials.json")
        .tempfile_in(parent)
        .map_err(|_| "Cannot create credential file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "Cannot protect credential file")?;
    }
    #[cfg(not(unix))]
    if !provider.api_key.is_empty() {
        return Err("Saving API keys is currently supported on macOS and Linux".into());
    }
    let data = serde_json::to_vec(provider).map_err(|_| "Cannot encode provider credentials")?;
    file.write_all(&data)
        .map_err(|_| "Cannot write provider credentials")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Cannot save provider credentials")?;
    file.persist(path)
        .map_err(|_| "Cannot replace provider credentials")?;
    Ok(())
}

pub const PROVIDER_ENVIRONMENT: [&str; 3] = [
    "S_CODE_MODEL_PROVIDER",
    "S_CODE_MODEL_BASE_URL",
    "S_CODE_MODEL_CREDENTIAL_HANDLE",
];

pub fn environment_managed() -> bool {
    PROVIDER_ENVIRONMENT
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn private_atomic_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/provider.json");
        let p = SavedProvider {
            provider: "openai_compatible".into(),
            base_url: "http://localhost:1/v1".into(),
            api_key: "test-private-value".into(),
            model: "model".into(),
        };
        save(&path, &p).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().api_key, p.api_key);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read(&path).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_without_overwriting_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "unchanged").unwrap();
        let path = dir.path().join("provider.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
    }
}

#[cfg(all(test, unix))]
mod integration_tests {
    use super::*;

    #[test]
    fn saved_setup_respects_effective_authentication_mode() {
        use crate::{Component, ConfigLoader};
        use serde_json::json;
        use std::os::unix::fs::PermissionsExt;

        // CLI > environment > file; a previous local setup must not reroute
        // managed credentials when any effective configuration uses Team Grant.
        let cases = [
            ("team_grant", None, None, false),
            ("development_token", Some("team_grant"), None, false),
            ("development_token", None, Some("team_grant"), false),
            (
                "team_grant",
                Some("development_token"),
                Some("team_grant"),
                false,
            ),
            ("development_token", None, None, true),
            ("team_grant", Some("development_token"), None, true),
            (
                "team_grant",
                Some("team_grant"),
                Some("development_token"),
                true,
            ),
        ];
        for component in [Component::Daemon, Component::Cli] {
            for (file_auth, env_auth, cli_auth, local) in cases {
                let dir = tempfile::tempdir().unwrap();
                let config = dir.path().join("config.json");
                fs::write(&config, json!({
                    "daemon": {
                        "auth_mode": file_auth,
                        "team_grant_key_id": "fixture",
                        "team_grant_public_key_base64": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                    },
                    "model": {
                        "provider": "openai_compatible",
                        "base_url": "https://managed.example/v1",
                        "credential_handle": "MANAGED_KEY"
                    },
                    "client": {"model": "managed-model"}
                }).to_string()).unwrap();
                fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
                let provider_path = config.with_extension("provider-credentials.json");
                save(
                    &provider_path,
                    &SavedProvider {
                        provider: "openai_compatible".into(),
                        base_url: "http://127.0.0.1:12345/v1".into(),
                        api_key: "synthetic-local-key".into(),
                        model: "personal-model".into(),
                    },
                )
                .unwrap();
                let load = || {
                    let mut loader = ConfigLoader::new().with_file(&config);
                    if let Some(auth) = env_auth {
                        loader = loader.with_environment([("S_CODE_DAEMON_AUTH_MODE", auth)]);
                    }
                    if let Some(auth) = cli_auth {
                        loader = loader.with_cli_override("daemon.auth_mode", json!(auth));
                    }
                    loader.load(component)
                };
                let effective = load().unwrap();
                assert_eq!(
                    effective.config.daemon.auth_mode,
                    if local {
                        "development_token"
                    } else {
                        "team_grant"
                    }
                );
                assert_eq!(
                    effective.config.model.base_url.as_deref(),
                    Some(if local {
                        "http://127.0.0.1:12345/v1"
                    } else {
                        "https://managed.example/v1"
                    })
                );
                assert_eq!(
                    effective.config.client.model,
                    if local {
                        "personal-model"
                    } else {
                        "managed-model"
                    }
                );
                assert_eq!(
                    effective.config.model.credential_handle.as_deref(),
                    Some("MANAGED_KEY")
                );
                assert!(
                    !effective
                        .redacted_json()
                        .to_string()
                        .contains("synthetic-local-key")
                );
                if !local {
                    assert!(
                        effective
                            .provenance
                            .values()
                            .all(|entry| entry.detail != "local provider setup")
                    );
                    // Ignored local credentials must not even be read: a corrupt
                    // leftover file cannot prevent a managed configuration loading.
                    fs::write(&provider_path, "invalid credentials file").unwrap();
                    assert!(load().is_ok());
                }
            }
        }
    }

    #[test]
    fn configuration_projects_no_secrets_and_environment_keeps_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        fs::write(&config, "schema_version = 1\n").unwrap();
        save(
            &config.with_extension("provider-credentials.json"),
            &SavedProvider {
                provider: "openai_compatible".into(),
                base_url: "https://api.sai.foundation/v1".into(),
                api_key: "unit-private-provider-key".into(),
                model: "chat-model".into(),
            },
        )
        .unwrap();
        let effective = crate::ConfigLoader::new()
            .with_file(&config)
            .load(crate::Component::Cli)
            .unwrap();
        assert_eq!(effective.config.client.model, "chat-model");
        assert!(
            !effective
                .redacted_json()
                .to_string()
                .contains("unit-private-provider-key")
        );
        let effective = crate::ConfigLoader::new()
            .with_file(config)
            .with_environment([("S_CODE_MODEL", "override-model")])
            .load(crate::Component::Cli)
            .unwrap();
        assert_eq!(effective.config.client.model, "override-model");
    }
}
