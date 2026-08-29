use anyhow::{Result, anyhow};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::process::{Child, Command, Stdio};
use url::Url;

const MAX_LINK_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptLink {
    pub(crate) url: String,
    pub(crate) label: String,
}

pub(crate) fn transcript_links(markdown: &str) -> Vec<TranscriptLink> {
    let mut current: Option<(String, String)> = None;
    let mut links = Vec::new();
    for event in Parser::new_ext(markdown, Options::ENABLE_GFM) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                current = Some((dest_url.into_string(), String::new()));
            }
            Event::Text(text) | Event::Code(text) if current.is_some() => {
                if let Some((_, label)) = current.as_mut() {
                    label.push_str(&text);
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some((url, label)) = current.take()
                    && validate_external_url(&url).is_ok()
                {
                    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                    links.push(TranscriptLink {
                        label: if label.is_empty() {
                            url.clone()
                        } else {
                            label.chars().take(96).collect()
                        },
                        url,
                    });
                }
            }
            _ => {}
        }
    }
    links
}

pub(crate) fn open_external_url(value: &str) -> Result<Child> {
    let url = validate_external_url(value)?;
    let mut command = platform_opener(url.as_str());
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| anyhow!("failed to open external link: {error}"))
}

fn validate_external_url(value: &str) -> Result<Url> {
    if value.len() > MAX_LINK_BYTES || value.chars().any(char::is_control) {
        return Err(anyhow!(
            "external link is empty, oversized, or contains controls"
        ));
    }
    let url = Url::parse(value).map_err(|_| anyhow!("external link is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(anyhow!(
            "external links must be credential-free HTTP(S) URLs"
        ));
    }
    Ok(url)
}

#[cfg(target_os = "macos")]
fn platform_opener(url: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command.arg(url);
    command
}

#[cfg(target_os = "windows")]
fn platform_opener(url: &str) -> Command {
    let mut command = Command::new("rundll32.exe");
    command.args(["url.dll,FileProtocolHandler", url]);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_opener(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(not(any(unix, target_os = "windows")))]
fn platform_opener(_url: &str) -> Command {
    Command::new("false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_bounded_credential_free_http_links() {
        let links = transcript_links(
            "[Docs](https://example.com/guide?q=1) \
             [unsafe](javascript:alert(1)) \
             [credential](https://user:secret@example.com/private) \
             <https://example.com/auto>",
        );
        assert_eq!(
            links,
            vec![
                TranscriptLink {
                    url: "https://example.com/guide?q=1".into(),
                    label: "Docs".into(),
                },
                TranscriptLink {
                    url: "https://example.com/auto".into(),
                    label: "https://example.com/auto".into(),
                },
            ]
        );
        assert!(validate_external_url("file:///etc/passwd").is_err());
        assert!(validate_external_url("https://example.com/\u{1b}").is_err());
    }
}
