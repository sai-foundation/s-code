//! Shared lexical protection for workspace file tools and Git content.

use std::path::Path;

const SENSITIVE_COMPONENTS: &[&str] = &[
    ".s-code",
    ".opencoding", // Legacy state remains private after the rename.
    ".ssh",
    ".aws",
    ".azure",
    ".docker",
    ".gnupg",
    ".kube",
    ".password-store",
    ".terraform.d",
];
const SENSITIVE_FILES: &[&str] = &[
    ".env",
    ".env.local",
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".openrouter_apikey",
    ".pypirc",
    "credentials",
    "credentials.json",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_ed25519_sk",
    "id_rsa",
    "key.json",
    "secrets.json",
    "service-account.json",
];
fn sensitive_name(lower: &str) -> bool {
    let environment_template = lower.starts_with(".env.")
        && [".example", ".sample", ".template"]
            .iter()
            .any(|suffix| lower.ends_with(suffix));
    SENSITIVE_COMPONENTS.contains(&lower)
        || SENSITIVE_FILES.contains(&lower)
        || lower.starts_with(".env.") && !environment_template
        || lower.ends_with(".key")
        || lower.ends_with(".pem")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.ends_with(".kdbx")
}

pub fn sensitive_path(path: &Path) -> bool {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if components.iter().any(|component| sensitive_name(component)) {
        return true;
    }
    components.windows(2).any(|parts| {
        parts[0] == ".config"
            && matches!(
                parts[1].as_str(),
                "gcloud" | "gh" | "hub" | "op" | "1password" | "s-code" | "opencoding"
            )
    }) || components
        .windows(3)
        .any(|parts| parts == [".local", "share", "keyrings"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_and_legacy_application_state_remain_sensitive() {
        for path in [
            ".s-code/state/sessions.sqlite",
            ".opencoding/state/sessions.sqlite",
            ".config/s-code/config.toml",
            ".config/opencoding/config.toml",
            "backup/.OPENCODING/storage-key",
        ] {
            assert!(sensitive_path(Path::new(path)), "{path}");
        }
        assert!(!sensitive_path(Path::new("src/s-code/client.rs")));
    }
}
