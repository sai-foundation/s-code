use futures_util::StreamExt;
use html5ever::{
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, CharacterTokens, EndTag, StartTag, Tag, TagToken, Token, TokenSink,
        TokenSinkResult, Tokenizer, states::RawKind,
    },
};
use reqwest::{
    Client, StatusCode,
    dns::{Addrs, Name, Resolve, Resolving},
    header::{
        ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap,
        HeaderValue, LOCATION,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    error::Error as StdError,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use url::{Host, Url};

const MAX_INPUT_URL_CHARS: usize = 4_096;
const MAX_URL_BYTES: usize = 512;
const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_PDF_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 24 * 1024;
const MAX_INLINE_RESULT_BYTES: usize = 30 * 1024;
const MAX_CONTENT_TYPE_BYTES: usize = 256;
const MAX_MEDIA_TYPE_BYTES: usize = 127;
const MAX_CHARSET_BYTES: usize = 32;
const MAX_ERROR_DETAIL_BYTES: usize = 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebOpenArgs {
    url: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebOpenResult {
    requested_url: String,
    final_url: String,
    status: u16,
    media_type: String,
    content: String,
    body_bytes: u64,
    sha256: String,
    truncated: bool,
    trust: &'static str,
}

#[derive(Debug, Error)]
pub(crate) enum WebOpenError {
    #[error("invalid public HTTPS URL: {0}")]
    InvalidUrl(String),
    #[error("public HTTPS connection failed: {0}")]
    Connection(String),
    #[error("public HTTPS request failed: {0}")]
    Request(String),
    #[error("public HTTPS request exceeded the 15 second limit")]
    Timeout,
    #[error("public HTTPS redirect failed: {0}")]
    Redirect(String),
    #[error("public HTTPS response returned HTTP {0}")]
    Status(u16),
    #[error("public HTTPS response media type is unsupported: {0}")]
    MediaType(String),
    #[error("public HTTPS response encoding is unsupported: {0}")]
    Encoding(String),
    #[error("public HTTPS response exceeds the {0} limit")]
    TooLarge(&'static str),
    #[error("public HTTPS response is not valid UTF-8")]
    InvalidUtf8,
    #[error("public HTTPS HTML could not be converted to text: {0}")]
    Html(String),
    #[error("public HTTPS result exceeds the inline result limit")]
    ResultTooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextKind {
    Html,
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchKind {
    Web,
    Pdf(&'static str),
}

impl FetchKind {
    fn tool(self) -> &'static str {
        match self {
            Self::Web => "web_open",
            Self::Pdf(tool) => tool,
        }
    }

    fn accept(self) -> &'static str {
        match self {
            Self::Web => {
                "text/html, application/xhtml+xml, text/plain, text/markdown, application/json, application/xml, text/xml"
            }
            Self::Pdf(_) => "application/pdf",
        }
    }

    fn max_body_bytes(self) -> usize {
        match self {
            Self::Web => MAX_BODY_BYTES,
            Self::Pdf(_) => MAX_PDF_BODY_BYTES,
        }
    }

    fn size_limit(self) -> &'static str {
        match self {
            Self::Web => "1 MiB",
            Self::Pdf(_) => "8 MiB",
        }
    }
}

pub(crate) struct PublicResponse {
    pub(crate) requested_url: String,
    pub(crate) final_url: String,
    pub(crate) status: u16,
    pub(crate) media_type: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug)]
struct PublicDnsResolver;

impl Resolve for PublicDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let resolved = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| resolver_error(format!("{host}: {error}")))?
                .collect::<Vec<_>>();
            validate_resolved_addresses(&host, &resolved).map_err(resolver_error)?;
            Ok(Box::new(resolved.into_iter()) as Addrs)
        })
    }
}

fn resolver_error(message: impl Into<String>) -> Box<dyn StdError + Send + Sync> {
    Box::new(io::Error::other(message.into()))
}

pub(crate) fn parse_arguments(value: &Value) -> Result<WebOpenArgs, WebOpenError> {
    let mut input: WebOpenArgs = serde_json::from_value(value.clone())
        .map_err(|error| WebOpenError::InvalidUrl(error_detail(error)))?;
    input.url = validate_url(&input.url)?.into();
    Ok(input)
}

pub(crate) fn normalized_arguments(value: &Value) -> Result<Value, WebOpenError> {
    let input = parse_arguments(value)?;
    Ok(serde_json::json!({"url": input.url}))
}

pub(crate) fn normalize_public_url(value: &str) -> Result<String, WebOpenError> {
    Ok(validate_url(value)?.into())
}

pub(crate) async fn open(input: WebOpenArgs) -> Result<WebOpenResult, WebOpenError> {
    tokio::time::timeout(TOTAL_TIMEOUT, open_inner(input))
        .await
        .map_err(|_| WebOpenError::Timeout)?
}

async fn open_inner(input: WebOpenArgs) -> Result<WebOpenResult, WebOpenError> {
    let client = client_builder(Arc::new(PublicDnsResolver))
        .build()
        .map_err(|error| WebOpenError::Request(error_detail(error)))?;
    open_with_client(input, &client).await
}

pub(crate) async fn download_pdf(
    url: &str,
    requesting_tool: &'static str,
) -> Result<PublicResponse, WebOpenError> {
    tokio::time::timeout(TOTAL_TIMEOUT, download_pdf_inner(url, requesting_tool))
        .await
        .map_err(|_| WebOpenError::Timeout)?
}

async fn download_pdf_inner(
    url: &str,
    requesting_tool: &'static str,
) -> Result<PublicResponse, WebOpenError> {
    let client = client_builder(Arc::new(PublicDnsResolver))
        .build()
        .map_err(|error| WebOpenError::Request(error_detail(error)))?;
    fetch_with_client(url, &client, FetchKind::Pdf(requesting_tool)).await
}

fn client_builder<R: Resolve + 'static>(resolver: Arc<R>) -> reqwest::ClientBuilder {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .referer(false)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .pool_max_idle_per_host(0)
        .dns_resolver(resolver)
        .user_agent(concat!("s-code/", env!("CARGO_PKG_VERSION"), " public-web"))
}

async fn open_with_client(
    input: WebOpenArgs,
    client: &Client,
) -> Result<WebOpenResult, WebOpenError> {
    let fetched = fetch_with_client(&input.url, client, FetchKind::Web).await?;
    let text = String::from_utf8(fetched.bytes.clone()).map_err(|_| WebOpenError::InvalidUtf8)?;
    let text_kind = response_media_type_from_value(&fetched.media_type, FetchKind::Web)?
        .expect("web media type always maps to text");
    let (extracted, extraction_truncated) = match text_kind {
        TextKind::Html => extract_html(text).await?,
        TextKind::Plain => (text, false),
    };
    let sha256 = format!("{:x}", Sha256::digest(&fetched.bytes));
    bounded_result(WebOpenResult {
        requested_url: fetched.requested_url,
        final_url: fetched.final_url,
        status: fetched.status,
        media_type: fetched.media_type,
        content: extracted,
        body_bytes: fetched.bytes.len() as u64,
        sha256,
        truncated: extraction_truncated,
        trust: "remote_untrusted",
    })
}

async fn fetch_with_client(
    url: &str,
    client: &Client,
    kind: FetchKind,
) -> Result<PublicResponse, WebOpenError> {
    let requested = validate_url(url)?;
    let mut current = requested.clone();
    let mut visited = BTreeSet::new();
    visited.insert(current.as_str().to_owned());

    for redirects in 0..=MAX_REDIRECTS {
        let response = client
            .get(current.clone())
            .headers(request_headers(kind))
            .send()
            .await
            .map_err(map_request_error)?;
        let status = response.status();
        if is_redirect(status) {
            if redirects == MAX_REDIRECTS {
                return Err(WebOpenError::Redirect(format!(
                    "more than {MAX_REDIRECTS} redirects"
                )));
            }
            let next = redirect_target(&current, response.headers())?;
            if next.origin() != current.origin() {
                return Err(cross_origin_redirect_error(&next, kind));
            }
            if !visited.insert(next.as_str().to_owned()) {
                return Err(WebOpenError::Redirect("redirect loop detected".into()));
            }
            current = next;
            continue;
        }
        if !status.is_success() {
            return Err(WebOpenError::Status(status.as_u16()));
        }
        validate_content_encoding(response.headers())?;
        let (media_type, _) = response_media_type(response.headers(), kind)?;
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > kind.max_body_bytes() as u64)
        {
            return Err(WebOpenError::TooLarge(kind.size_limit()));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_request_error)?;
            if bytes.len().saturating_add(chunk.len()) > kind.max_body_bytes() {
                return Err(WebOpenError::TooLarge(kind.size_limit()));
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(PublicResponse {
            requested_url: requested.as_str().to_owned(),
            final_url: current.as_str().to_owned(),
            status: status.as_u16(),
            media_type,
            bytes,
        });
    }
    unreachable!("redirect loop returns from every bounded path")
}

fn cross_origin_redirect_error(next: &Url, kind: FetchKind) -> WebOpenError {
    WebOpenError::Redirect(format!(
        "redirect changes origin to {}; submit that URL as a new {} request",
        next.as_str(),
        kind.tool(),
    ))
}

fn map_request_error(error: reqwest::Error) -> WebOpenError {
    if error.is_timeout() {
        WebOpenError::Timeout
    } else if error.is_connect() {
        WebOpenError::Connection(error_detail(format!("{error:?}")))
    } else {
        WebOpenError::Request(error_detail(format!("{error:?}")))
    }
}

fn request_headers(kind: FetchKind) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static(kind.accept()));
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    headers
}

fn validate_url(value: &str) -> Result<Url, WebOpenError> {
    if value.is_empty()
        || value.chars().count() > MAX_INPUT_URL_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(WebOpenError::InvalidUrl(
            "URL must contain 1 to 4096 printable characters".into(),
        ));
    }
    let mut url = Url::parse(value)
        .map_err(|_| WebOpenError::InvalidUrl("URL must be an absolute HTTPS address".into()))?;
    if url.scheme() != "https" {
        return Err(WebOpenError::InvalidUrl(
            "only HTTPS addresses are supported".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebOpenError::InvalidUrl(
            "embedded usernames and passwords are not supported".into(),
        ));
    }
    if url.port_or_known_default() != Some(443) {
        return Err(WebOpenError::InvalidUrl(
            "only the standard HTTPS port 443 is supported".into(),
        ));
    }
    let host = url
        .host()
        .ok_or_else(|| WebOpenError::InvalidUrl("URL must include a host".into()))?;
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain.is_empty()
                || domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || domain.ends_with(".internal")
                || domain == "home.arpa"
                || domain.ends_with(".home.arpa")
            {
                return Err(WebOpenError::InvalidUrl(
                    "local and private host names are not supported".into(),
                ));
            }
        }
        Host::Ipv4(address) if !is_public_ip(IpAddr::V4(address)) => {
            return Err(WebOpenError::InvalidUrl(
                "private and special-use IP addresses are not supported".into(),
            ));
        }
        Host::Ipv6(address) if !is_public_ip(IpAddr::V6(address)) => {
            return Err(WebOpenError::InvalidUrl(
                "private and special-use IP addresses are not supported".into(),
            ));
        }
        Host::Ipv4(_) | Host::Ipv6(_) => {}
    }
    url.set_fragment(None);
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(WebOpenError::InvalidUrl(format!(
            "normalized URL must not exceed {MAX_URL_BYTES} bytes"
        )));
    }
    if s_code_audit::redact_text(url.as_str()) != url.as_str() {
        return Err(WebOpenError::InvalidUrl(
            "URLs containing credentials or detected secrets are not supported".into(),
        ));
    }
    Ok(url)
}

fn is_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn redirect_target(current: &Url, headers: &HeaderMap) -> Result<Url, WebOpenError> {
    let location = headers
        .get_all(LOCATION)
        .iter()
        .map(|value| value.to_str())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WebOpenError::Redirect("Location is not valid text".into()))?;
    if location.len() != 1 {
        return Err(WebOpenError::Redirect(
            "redirect must provide exactly one Location".into(),
        ));
    }
    let target = current
        .join(location[0])
        .map_err(|_| WebOpenError::Redirect("Location is not a valid URL".into()))?;
    validate_url(target.as_str()).map_err(|error| WebOpenError::Redirect(error.to_string()))
}

fn validate_content_encoding(headers: &HeaderMap) -> Result<(), WebOpenError> {
    let encodings = headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .map(|value| value.to_str())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WebOpenError::Encoding("invalid Content-Encoding".into()))?;
    if encodings
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|value| !value.is_empty() && !value.eq_ignore_ascii_case("identity"))
    {
        return Err(WebOpenError::Encoding(
            "compressed responses are not accepted".into(),
        ));
    }
    Ok(())
}

fn response_media_type(
    headers: &HeaderMap,
    expected: FetchKind,
) -> Result<(String, Option<TextKind>), WebOpenError> {
    let values = headers
        .get_all(CONTENT_TYPE)
        .iter()
        .map(|value| value.to_str())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| WebOpenError::MediaType("invalid Content-Type".into()))?;
    if values.len() != 1 {
        return Err(WebOpenError::MediaType(
            "exactly one Content-Type is required".into(),
        ));
    }
    if values[0].len() > MAX_CONTENT_TYPE_BYTES {
        return Err(WebOpenError::MediaType(
            "Content-Type header is too long".into(),
        ));
    }
    let mut parts = values[0].split(';');
    let media_type = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    if media_type.len() > MAX_MEDIA_TYPE_BYTES {
        return Err(WebOpenError::MediaType("media type is too long".into()));
    }
    for parameter in parts {
        let Some((name, value)) = parameter.split_once('=') else {
            return Err(WebOpenError::MediaType(
                "invalid Content-Type parameter".into(),
            ));
        };
        if name.trim().eq_ignore_ascii_case("charset") {
            let charset = value.trim().trim_matches('"');
            if charset.len() > MAX_CHARSET_BYTES {
                return Err(WebOpenError::Encoding("charset value is too long".into()));
            }
            if !matches!(
                charset.to_ascii_lowercase().as_str(),
                "utf-8" | "utf8" | "us-ascii"
            ) {
                return Err(WebOpenError::Encoding(format!(
                    "charset {charset} is not supported"
                )));
            }
        }
    }
    let kind = response_media_type_from_value(&media_type, expected)?;
    Ok((media_type, kind))
}

fn response_media_type_from_value(
    media_type: &str,
    expected: FetchKind,
) -> Result<Option<TextKind>, WebOpenError> {
    match (expected, media_type) {
        (FetchKind::Pdf(_), "application/pdf") => Ok(None),
        (FetchKind::Pdf(_), value) => Err(WebOpenError::MediaType(value.into())),
        (FetchKind::Web, "text/html" | "application/xhtml+xml") => Ok(Some(TextKind::Html)),
        (
            FetchKind::Web,
            "text/plain" | "text/markdown" | "text/xml" | "application/json" | "application/xml",
        ) => Ok(Some(TextKind::Plain)),
        (FetchKind::Web, value)
            if value.starts_with("application/")
                && (value.ends_with("+json") || value.ends_with("+xml")) =>
        {
            Ok(Some(TextKind::Plain))
        }
        (FetchKind::Web, "application/pdf") => Err(WebOpenError::MediaType(
            "application/pdf (use pdf_read for bounded PDF text)".into(),
        )),
        (_, value) => Err(WebOpenError::MediaType(value.into())),
    }
}

fn validate_resolved_addresses(host: &str, addresses: &[SocketAddr]) -> Result<(), String> {
    if addresses.is_empty() {
        return Err(format!("{host} returned no addresses"));
    }
    if addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(format!(
            "{host} resolved to a private or special-use address"
        ));
    }
    Ok(())
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    const DENIED: &[(u32, u8)] = &[
        (0x0000_0000, 8),
        (0x0a00_0000, 8),
        (0x6440_0000, 10),
        (0x7f00_0000, 8),
        (0xa9fe_0000, 16),
        (0xac10_0000, 12),
        (0xc000_0000, 24),
        (0xc000_0200, 24),
        (0xc058_6300, 24),
        (0xc0a8_0000, 16),
        (0xc612_0000, 15),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
        (0xe000_0000, 4),
        (0xf000_0000, 4),
    ];
    let value = u32::from(address);
    !DENIED
        .iter()
        .any(|(network, prefix)| in_ipv4_prefix(value, *network, *prefix))
}

fn in_ipv4_prefix(value: u32, network: u32, prefix: u8) -> bool {
    let mask = u32::MAX << (32 - prefix);
    value & mask == network & mask
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let value = u128::from(address);
    if !in_ipv6_prefix(value, 0x2000_u128 << 112, 3) {
        return false;
    }
    const DENIED: &[(u128, u8)] = &[
        (0x2001_0000_u128 << 96, 23),
        (0x2001_0db8_u128 << 96, 32),
        (0x2002_u128 << 112, 16),
        (0x3fff_u128 << 112, 20),
    ];
    !DENIED
        .iter()
        .any(|(network, prefix)| in_ipv6_prefix(value, *network, *prefix))
}

fn in_ipv6_prefix(value: u128, network: u128, prefix: u8) -> bool {
    let mask = u128::MAX << (128 - prefix);
    value & mask == network & mask
}

#[derive(Default)]
struct HtmlTextCollector {
    content: String,
    pending_space: bool,
    truncated: bool,
}

impl HtmlTextCollector {
    fn separator(&mut self) {
        self.pending_space = false;
        while self.content.ends_with(' ') {
            self.content.pop();
        }
        if !self.content.is_empty() && !self.content.ends_with('\n') {
            self.content.push('\n');
        }
    }

    fn text_chunk(&mut self, value: &str) {
        for character in value.chars() {
            if character.is_whitespace() {
                self.pending_space = !self.content.is_empty();
                continue;
            }
            if self.content.len() >= MAX_TEXT_BYTES {
                self.truncated = true;
                break;
            }
            if self.pending_space && !self.content.ends_with('\n') {
                self.content.push(' ');
            }
            self.pending_space = false;
            if self.content.len().saturating_add(character.len_utf8()) > MAX_TEXT_BYTES {
                self.truncated = true;
                break;
            }
            self.content.push(character);
        }
    }

    /// Length of the text so far, without the line break that may end it.
    fn text_len(&self) -> usize {
        self.content.trim_end_matches('\n').len()
    }

    /// Writes the target of a link after the link's text, on the same line
    /// even where a block inside the link has already ended that line.
    fn link_target(&mut self, link: &OpenLink) {
        let line_ended = self.content.ends_with('\n') && self.text_len() > link.text_start;
        if line_ended {
            self.content.pop();
        }
        self.text_chunk(&format!(" [{}]", link.href));
        if line_ended {
            self.separator();
        }
    }

    fn finish(self) -> (String, bool) {
        (self.content.trim().to_owned(), self.truncated)
    }
}

/// A link whose end tag has not arrived yet.
struct OpenLink {
    href: String,
    /// `HtmlTextCollector::text_len` where the link began.
    text_start: usize,
}

#[derive(Default)]
struct HtmlSinkState {
    collector: HtmlTextCollector,
    excluded: Vec<String>,
    links: Vec<OpenLink>,
}

impl HtmlSinkState {
    /// Whether the innermost skipped element is a graphic. Its tags follow the
    /// HTML standard's rules for foreign content instead of those for HTML.
    fn in_foreign_content(&self) -> bool {
        self.excluded
            .last()
            .is_some_and(|open| is_foreign_html_element(open))
    }

    fn leave_foreign_content(&mut self) {
        while self.in_foreign_content() {
            self.excluded.pop();
        }
    }

    /// Ends the skipped element `name` together with what its end tag closes
    /// in a browser: graphics left open inside it, and for an HTML element
    /// also the HTML those graphics hold. Any other end tag is ignored.
    fn close_excluded(&mut self, name: &str) {
        let Some(position) = self.excluded.iter().rposition(|open| open == name) else {
            return;
        };
        let html_end_tag = is_excluded_html_element(name) && !is_foreign_html_element(name);
        let closes = self.excluded[position + 1..].iter().all(|open| {
            is_foreign_html_element(open) || (html_end_tag && !is_excluded_html_element(open))
        });
        if closes {
            self.excluded.truncate(position);
        }
    }
}

#[derive(Default)]
struct HtmlTextSink {
    state: RefCell<HtmlSinkState>,
}

impl HtmlTextSink {
    fn finish(self) -> (String, bool) {
        self.state.into_inner().collector.finish()
    }
}

impl TokenSink for HtmlTextSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut state = self.state.borrow_mut();
        match token {
            CharacterTokens(text) if state.excluded.is_empty() => {
                state.collector.text_chunk(text.as_ref());
            }
            TagToken(tag) if tag.kind == StartTag => {
                let name: &str = tag.name.as_ref();
                // A graphic that was never closed ends at ordinary HTML.
                if state.in_foreign_content() && leaves_foreign_content(&tag) {
                    state.leave_foreign_content();
                }
                let foreign = state.in_foreign_content();
                // Only a graphic and the elements inside it honor the slash.
                // Every HTML element ignores it and waits for its end tag.
                if tag.self_closing && (foreign || is_foreign_html_element(name)) {
                    return TokenSinkResult::Continue;
                }
                if is_excluded_html_element(name) || (foreign && is_html_integration_point(&tag)) {
                    state.excluded.push(name.to_owned());
                } else if state.excluded.is_empty() {
                    if is_block_html_element(name) {
                        state.collector.separator();
                    }
                    if name == "a" {
                        let href = tag
                            .attrs
                            .iter()
                            .find(|attribute| {
                                let local: &str = attribute.name.local.as_ref();
                                local == "href"
                            })
                            .map(|attribute| attribute.value.to_string())
                            .filter(|value| !value.is_empty() && !value.starts_with('#'))
                            .unwrap_or_default();
                        let link = OpenLink {
                            href,
                            text_start: state.collector.text_len(),
                        };
                        if tag.self_closing {
                            if !link.href.is_empty() {
                                state.collector.link_target(&link);
                            }
                        } else {
                            state.links.push(link);
                        }
                    }
                }
                return html_raw_text_mode(name);
            }
            TagToken(tag) if tag.kind == EndTag => {
                let name: &str = tag.name.as_ref();
                if state.in_foreign_content() && matches!(name, "br" | "p") {
                    state.leave_foreign_content();
                }
                if !state.excluded.is_empty() {
                    state.close_excluded(name);
                } else if is_block_html_element(name) {
                    // Whatever follows a block starts a new line.
                    state.collector.separator();
                } else if name == "a"
                    && let Some(link) = state.links.pop()
                    && !link.href.is_empty()
                {
                    state.collector.link_target(&link);
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn is_excluded_html_element(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "template"
            | "noscript"
            | "noembed"
            | "noframes"
            | "iframe"
            | "svg"
            | "math"
    )
}

fn is_foreign_html_element(name: &str) -> bool {
    matches!(name, "svg" | "math")
}

/// Elements inside a graphic whose children are ordinary HTML. That HTML
/// belongs to the graphic, so it does not end it.
fn is_html_integration_point(tag: &Tag) -> bool {
    let name: &str = tag.name.as_ref();
    match name {
        "foreignobject" | "desc" | "title" | "mi" | "mo" | "mn" | "ms" | "mtext" => true,
        "annotation-xml" => tag.attrs.iter().any(|attribute| {
            let local: &str = attribute.name.local.as_ref();
            local == "encoding"
                && ["text/html", "application/xhtml+xml"]
                    .iter()
                    .any(|encoding| attribute.value.eq_ignore_ascii_case(encoding))
        }),
        _ => false,
    }
}

/// Start tags that end an unclosed graphic under the HTML standard's rules
/// for parsing tokens in foreign content.
fn leaves_foreign_content(tag: &Tag) -> bool {
    let name: &str = tag.name.as_ref();
    match name {
        "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd" | "div" | "dl"
        | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "head" | "hr" | "i"
        | "img" | "li" | "listing" | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby"
        | "s" | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table" | "tt" | "u"
        | "ul" | "var" => true,
        "font" => tag.attrs.iter().any(|attribute| {
            let local: &str = attribute.name.local.as_ref();
            matches!(local, "color" | "face" | "size")
        }),
        _ => false,
    }
}

/// Elements whose text stands on lines of its own: those the HTML standard
/// displays as a block, a list item or a part of a table, and the line break.
/// So does the title, which a browser shows outside the page.
fn is_block_html_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "br"
            | "caption"
            | "center"
            | "col"
            | "colgroup"
            | "dd"
            | "details"
            | "dialog"
            | "dir"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "html"
            | "legend"
            | "li"
            | "listing"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "optgroup"
            | "option"
            | "p"
            | "plaintext"
            | "pre"
            | "search"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "title"
            | "tr"
            | "ul"
            | "xmp"
    )
}

fn html_raw_text_mode(name: &str) -> TokenSinkResult<()> {
    match name {
        "script" => TokenSinkResult::RawData(RawKind::ScriptData),
        "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
            TokenSinkResult::RawData(RawKind::Rawtext)
        }
        "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
        "plaintext" => TokenSinkResult::Plaintext,
        _ => TokenSinkResult::Continue,
    }
}

async fn extract_html(html: String) -> Result<(String, bool), WebOpenError> {
    tokio::task::spawn_blocking(move || {
        catch_unwind(AssertUnwindSafe(|| extract_html_sync(html.as_bytes())))
            .map_err(|_| WebOpenError::Html("parser panicked on malformed HTML".into()))?
    })
    .await
    .map_err(|error| WebOpenError::Html(error_detail(format!("parser task failed: {error}"))))?
}

fn extract_html_sync(html: &[u8]) -> Result<(String, bool), WebOpenError> {
    let html = std::str::from_utf8(html).map_err(|_| WebOpenError::InvalidUtf8)?;
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let tokenizer = Tokenizer::new(HtmlTextSink::default(), Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    Ok(tokenizer.sink.finish())
}

fn bounded_result(mut result: WebOpenResult) -> Result<WebOpenResult, WebOpenError> {
    let (content, truncated) = truncate_utf8(&result.content, MAX_TEXT_BYTES);
    result.content = content;
    result.truncated |= truncated;
    loop {
        let encoded = serde_json::to_vec(&result).expect("web result serialization is infallible");
        if encoded.len() <= MAX_INLINE_RESULT_BYTES {
            return Ok(result);
        }
        if result.content.is_empty() {
            return Err(WebOpenError::ResultTooLarge);
        }
        let target = result
            .content
            .len()
            .saturating_sub(encoded.len() - MAX_INLINE_RESULT_BYTES + 64);
        let (content, _) = truncate_utf8(&result.content, target);
        result.content = content;
        result.truncated = true;
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_owned(), false);
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

fn error_detail(value: impl std::fmt::Display) -> String {
    let value = value.to_string();
    let (mut value, truncated) = truncate_utf8(&value, MAX_ERROR_DETAIL_BYTES);
    if truncated {
        value.push('…');
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertifiedKey, generate_simple_self_signed};
    use reqwest::header::HeaderName;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsAcceptor;

    #[derive(Debug)]
    struct FixtureResolver {
        address: SocketAddr,
        lookups: Arc<Mutex<Vec<String>>>,
        reject_after: Option<usize>,
    }

    impl Resolve for FixtureResolver {
        fn resolve(&self, name: Name) -> Resolving {
            let host = name.as_str().to_owned();
            let address = self.address;
            let lookups = Arc::clone(&self.lookups);
            let reject_after = self.reject_after;
            Box::pin(async move {
                let count = {
                    let mut lookups = lookups.lock().expect("fixture lookup lock");
                    let count = lookups.len();
                    lookups.push(host.clone());
                    count
                };
                if reject_after.is_some_and(|limit| count >= limit) {
                    return Err(resolver_error(format!(
                        "{host} changed to a private address"
                    )));
                }
                Ok(Box::new(std::iter::once(address)) as Addrs)
            })
        }
    }

    struct HttpsFixture {
        address: SocketAddr,
        root: reqwest::Certificate,
        requests: Arc<tokio::sync::Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for HttpsFixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn https_fixture(responses: Vec<Vec<u8>>) -> HttpsFixture {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(["web.test".to_owned()]).unwrap();
        let certificate = cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der()));
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            for response in responses {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 || request.len() > 16 * 1024 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                captured
                    .lock()
                    .await
                    .push(String::from_utf8_lossy(&request).into_owned());
                stream.write_all(&response).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        HttpsFixture {
            address,
            root: reqwest::Certificate::from_der(certificate.as_ref()).unwrap(),
            requests,
            task,
        }
    }

    fn fixture_client(
        fixture: &HttpsFixture,
        reject_after: Option<usize>,
    ) -> (Client, Arc<Mutex<Vec<String>>>) {
        let lookups = Arc::new(Mutex::new(Vec::new()));
        let resolver = FixtureResolver {
            address: fixture.address,
            lookups: Arc::clone(&lookups),
            reject_after,
        };
        let client = client_builder(Arc::new(resolver))
            .add_root_certificate(fixture.root.clone())
            .build()
            .unwrap();
        (client, lookups)
    }

    #[test]
    fn url_validation_accepts_only_public_https_targets() {
        let accepted = [
            "https://example.com/path?q=one#section",
            "https://1.1.1.1/",
            "https://[2606:4700:4700::1111]/",
            "https://example.com:443/",
        ];
        for value in accepted {
            let parsed = validate_url(value).unwrap_or_else(|error| panic!("{value}: {error}"));
            assert!(parsed.fragment().is_none());
        }
        for value in [
            "http://example.com",
            "file:///etc/passwd",
            "https://user:password@example.com",
            "https://example.com/?api_key=sk-project-1234567890",
            "https://example.com:8443",
            "https://localhost",
            "https://service.internal",
            "https://127.0.0.1",
            "https://127.1",
            "https://2130706433",
            "https://0x7f000001",
            "https://0177.0.0.1",
            "https://[::1]",
            "https://[::ffff:127.0.0.1]",
        ] {
            assert!(validate_url(value).is_err(), "accepted unsafe URL {value}");
        }
        assert!(validate_url(&format!("https://example.com/{}", "é".repeat(200))).is_err());
        assert_eq!(
            validate_url("https://bücher.example/a/../docs#part")
                .unwrap()
                .as_str(),
            "https://xn--bcher-kva.example/docs"
        );
    }

    #[test]
    fn public_address_filter_rejects_special_and_mixed_answers() {
        for value in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "fc00::1",
            "fe80::1",
            "2001:5::1",
            "2001:db8::1",
            "2002:0808:0808::1",
            "3fff::1",
            "ff02::1",
        ] {
            let address: IpAddr = value.parse().unwrap();
            assert!(!is_public_ip(address), "accepted special address {value}");
        }
        for value in [
            "1.1.1.1",
            "8.8.8.8",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
        ] {
            assert!(
                is_public_ip(value.parse().unwrap()),
                "rejected public {value}"
            );
        }
        assert!(
            validate_resolved_addresses(
                "example.com",
                &[
                    SocketAddr::new("1.1.1.1".parse().unwrap(), 0),
                    SocketAddr::new("127.0.0.1".parse().unwrap(), 0),
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn redirects_are_revalidated_and_loose_locations_are_rejected() {
        let current = Url::parse("https://example.com/docs/start").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(LOCATION, HeaderValue::from_static("../next#part"));
        assert_eq!(
            redirect_target(&current, &headers).unwrap().as_str(),
            "https://example.com/next"
        );
        headers.insert(
            LOCATION,
            HeaderValue::from_static("http://example.com/plain"),
        );
        assert!(redirect_target(&current, &headers).is_err());
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://127.0.0.1/private"),
        );
        assert!(redirect_target(&current, &headers).is_err());
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://other.example/next"),
        );
        let cross_origin = redirect_target(&current, &headers).unwrap();
        assert_ne!(cross_origin.origin(), current.origin());
        assert!(matches!(
            cross_origin_redirect_error(&cross_origin, FetchKind::Pdf("pdf_view")),
            WebOpenError::Redirect(message)
                if message.contains("new pdf_view request")
        ));
    }

    #[test]
    fn fixed_request_headers_cannot_carry_credentials_or_compressed_content() {
        for kind in [FetchKind::Web, FetchKind::Pdf("pdf_read")] {
            let headers = request_headers(kind);
            assert_eq!(headers[ACCEPT_ENCODING], "identity");
            for name in ["authorization", "cookie", "referer", "proxy-authorization"] {
                assert!(!headers.contains_key(HeaderName::from_static(name)));
            }
        }
    }

    #[tokio::test]
    async fn https_transport_sends_fixed_headers_and_follows_same_origin_redirects() {
        let body = b"<html><body><h1>Fixture</h1><p>safe &amp; readable</p></body></html>";
        let fixture = https_fixture(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            )
            .into_bytes(),
        ])
        .await;
        let (client, lookups) = fixture_client(&fixture, None);
        let input = parse_arguments(&serde_json::json!({
            "url":"https://web.test/start?topic=rust"
        }))
        .unwrap();
        let result = open_with_client(input, &client).await.unwrap();
        assert_eq!(result.final_url, "https://web.test/final");
        assert_eq!(result.media_type, "text/html");
        assert!(result.content.contains("Fixture"));
        assert!(result.content.contains("safe & readable"));
        assert_eq!(result.body_bytes, body.len() as u64);
        assert_eq!(result.sha256, format!("{:x}", Sha256::digest(body)));
        assert!(!result.truncated);
        assert_eq!(lookups.lock().unwrap().as_slice(), ["web.test", "web.test"]);

        let requests = fixture.requests.lock().await;
        assert_eq!(requests.len(), 2);
        let first = requests[0].to_ascii_lowercase();
        assert!(first.starts_with("get /start?topic=rust http/1.1\r\n"));
        assert!(first.contains("\r\nhost: web.test\r\n"));
        assert!(first.contains("\r\naccept-encoding: identity\r\n"));
        assert!(first.contains("\r\nuser-agent: s-code/"));
        for header in [
            "authorization:",
            "cookie:",
            "referer:",
            "proxy-authorization:",
        ] {
            assert!(
                !first.contains(header),
                "unexpected request header {header}"
            );
        }
    }

    #[tokio::test]
    async fn pdf_transport_reuses_the_credential_free_public_https_boundary() {
        let body = b"%PDF-1.7\nfixture";
        let fixture = https_fixture(vec![
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            )
            .into_bytes(),
        ])
        .await;
        let (client, lookups) = fixture_client(&fixture, None);
        let result = fetch_with_client(
            "https://web.test/paper.pdf#page=2",
            &client,
            FetchKind::Pdf("pdf_read"),
        )
        .await
        .unwrap();
        assert_eq!(result.requested_url, "https://web.test/paper.pdf");
        assert_eq!(result.final_url, "https://web.test/paper.pdf");
        assert_eq!(result.media_type, "application/pdf");
        assert_eq!(result.bytes, body);
        assert_eq!(lookups.lock().unwrap().as_slice(), ["web.test"]);
        let request = fixture.requests.lock().await[0].to_ascii_lowercase();
        assert!(request.contains("\r\naccept: application/pdf\r\n"));
        for header in [
            "authorization:",
            "cookie:",
            "referer:",
            "proxy-authorization:",
        ] {
            assert!(
                !request.contains(header),
                "unexpected request header {header}"
            );
        }
    }

    #[tokio::test]
    async fn redirects_cannot_bypass_origin_approval_or_dns_revalidation() {
        let cross_origin = https_fixture(vec![
            b"HTTP/1.1 302 Found\r\nLocation: https://other.test/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ])
        .await;
        let (client, lookups) = fixture_client(&cross_origin, None);
        let input = parse_arguments(&serde_json::json!({"url":"https://web.test/start"})).unwrap();
        let error = open_with_client(input, &client).await.unwrap_err();
        assert!(matches!(error, WebOpenError::Redirect(message) if message.contains("other.test")));
        assert_eq!(lookups.lock().unwrap().as_slice(), ["web.test"]);
        assert_eq!(cross_origin.requests.lock().await.len(), 1);

        let rebound = https_fixture(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nwrong".to_vec(),
        ])
        .await;
        let (client, lookups) = fixture_client(&rebound, Some(1));
        let input = parse_arguments(&serde_json::json!({"url":"https://web.test/start"})).unwrap();
        assert!(matches!(
            open_with_client(input, &client).await,
            Err(WebOpenError::Connection(_))
        ));
        assert_eq!(lookups.lock().unwrap().as_slice(), ["web.test", "web.test"]);
        assert_eq!(rebound.requests.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn transport_rejects_oversized_and_encoded_responses_before_body_delivery() {
        let oversized = https_fixture(vec![
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_BODY_BYTES + 1
            )
            .into_bytes(),
        ])
        .await;
        let (client, _) = fixture_client(&oversized, None);
        let input = parse_arguments(&serde_json::json!({"url":"https://web.test/large"})).unwrap();
        assert!(matches!(
            open_with_client(input, &client).await,
            Err(WebOpenError::TooLarge("1 MiB"))
        ));

        let oversized_pdf = https_fixture(vec![
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_PDF_BODY_BYTES + 1
            )
            .into_bytes(),
        ])
        .await;
        let (client, _) = fixture_client(&oversized_pdf, None);
        assert!(matches!(
            fetch_with_client(
                "https://web.test/large.pdf",
                &client,
                FetchKind::Pdf("pdf_read"),
            )
            .await,
            Err(WebOpenError::TooLarge("8 MiB"))
        ));

        let encoded = https_fixture(vec![
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnope".to_vec(),
        ])
        .await;
        let (client, _) = fixture_client(&encoded, None);
        let input = parse_arguments(&serde_json::json!({"url":"https://web.test/gzip"})).unwrap();
        assert!(matches!(
            open_with_client(input, &client).await,
            Err(WebOpenError::Encoding(_))
        ));
    }

    #[test]
    fn response_type_and_encoding_are_strict() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        assert_eq!(
            response_media_type(&headers, FetchKind::Web).unwrap().1,
            Some(TextKind::Html)
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        assert_eq!(
            response_media_type(&headers, FetchKind::Web).unwrap().1,
            Some(TextKind::Plain)
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
        assert!(response_media_type(&headers, FetchKind::Web).is_err());
        assert_eq!(
            response_media_type(&headers, FetchKind::Pdf("pdf_read"))
                .unwrap()
                .1,
            None
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/png"));
        assert!(response_media_type(&headers, FetchKind::Web).is_err());
        assert!(response_media_type(&headers, FetchKind::Pdf("pdf_read")).is_err());
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=latin1"),
        );
        assert!(response_media_type(&headers, FetchKind::Web).is_err());
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&format!("application/{}+json", "x".repeat(300))).unwrap(),
        );
        assert!(response_media_type(&headers, FetchKind::Web).is_err());
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_str(&format!("text/plain; charset={}", "x".repeat(100))).unwrap(),
        );
        assert!(response_media_type(&headers, FetchKind::Web).is_err());

        let mut encoded = HeaderMap::new();
        encoded.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(validate_content_encoding(&encoded).is_err());
    }

    #[test]
    fn html_is_rendered_without_script_and_results_stay_inline() {
        let html = b"<html><head><style>hidden</style></head><body><h1>Title</h1><script>bad()</script><p>Hello <a href='https://example.com'>world</a>.</p></body></html>";
        let (rendered, truncated) = extract_html_sync(html).unwrap();
        assert!(!truncated);
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("world"));
        assert!(rendered.contains("https://example.com"));
        assert!(!rendered.contains("bad()"));
        let result = bounded_result(WebOpenResult {
            requested_url: format!("https://example.com/{}", "a".repeat(400)),
            final_url: format!("https://example.com/{}", "b".repeat(400)),
            status: 200,
            media_type: "text/html".into(),
            content: "x".repeat(100_000),
            body_bytes: 100_000,
            sha256: "a".repeat(64),
            truncated: false,
            trust: "remote_untrusted",
        })
        .unwrap();
        assert!(result.truncated);
        assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_INLINE_RESULT_BYTES);
    }

    #[test]
    fn malformed_table_html_does_not_panic_the_extractor() {
        let html = br###"/p>
<table cellpadding="0" style="border-style:none;font-size:90%;border-collapse:separate;border-spacing:0;margin:1em 2em 1em 1em"><td colspan="3" style="text-align:center;border:1px solid var(--border-color-base,#a2a9b1);color:var(--color-base,#202122);overflow:inherit;background-color:var(--background-color-neutral,#eaecf0)"><td rowspan="3" colspan="5" style="border-style:solid;border-width:0;border-color:inherit"><tr><td rowspan="2" style="border-style:solid;border-width:0;border-color:inherit;border-bottom-width:0"><tr><td rowspan="2" style="border-style:solid;border-width:0;border-color:inherit;border-top-width:0"><tr><td style="height:7px"><td rowspan="2" style="color:var(--color-base,#202122);overflow:inherit;background-color:var(--background-color-neutral-subtle,#f8f9fa);padding:0 2px;border:1px solid var(--border-color-base,#a2a9b1);border-top-width:1px"><b>NY Rangers<td rowspan="2" colspan="8" style="text-align:center"><b><a href="Eastern_Conference_(NHL)" title="Eastern Conference (NHL)">Eastern Conference<tr><td style="height:7px"><td style="border-color:inherit;border-width:0 0px 0 0;border-style:solid"><"###;
        let (rendered, _) = extract_html_sync(html).unwrap();
        assert!(rendered.contains("NY Rangers"));
        assert!(rendered.contains("Eastern Conference"));
    }

    #[test]
    fn html_extractor_excludes_foreign_active_content_and_decodes_only_entities() {
        let html = br#"<svg><script>svg_bad()</script><text>hidden graphic</text></svg><math><annotation>hidden math</annotation></math><p>good &amp; safe</p>"#;
        let (rendered, _) = extract_html_sync(html).unwrap();
        assert_eq!(rendered, "good & safe");

        let (plain, _) = extract_html_sync(b"<plaintext>&amp; stays literal").unwrap();
        assert_eq!(plain, "&amp; stays literal");
    }

    #[test]
    fn html_extractor_resumes_where_a_browser_ends_a_graphic() {
        for (html, expected) in [
            // A self-closing graphic is complete.
            ("<p>one</p><svg/><p>two</p>", "one\ntwo"),
            ("<p>one</p><math/><p>two</p>", "one\ntwo"),
            (
                r#"<a href="https://example.com/"><svg width="16"/>home</a><p>two</p>"#,
                "home [https://example.com/]\ntwo",
            ),
            // So is a self-closing element inside a graphic.
            ("<svg><script/><text>label</text></svg><p>two</p>", "two"),
            ("<svg><title/><style/></svg><p>two</p>", "two"),
            // Ordinary HTML ends a graphic that was never closed.
            (
                "<p>one</p><svg><g><text>label</text></g><p>two</p>",
                "one\ntwo",
            ),
            ("<p>one</p><math><mrow><div>two</div>", "one\ntwo"),
            (
                r#"<svg><font>label</font><font color="red">two</font>"#,
                "two",
            ),
            // The end of an enclosing element ends a graphic left open in it.
            ("<template><svg></template><p>after</p>", "after"),
        ] {
            let (rendered, truncated) = extract_html_sync(html.as_bytes()).unwrap();
            assert_eq!(rendered, expected, "{html}");
            assert!(!truncated, "{html}");
        }
    }

    #[test]
    fn html_extractor_keeps_skipping_what_belongs_to_a_skipped_element() {
        for (html, expected) in [
            // Ordinary elements ignore the slash and wait for their end tag.
            ("<p>one</p><script/><p>two</p>", "one"),
            ("<p>one</p><style/><p>two</p>", "one"),
            ("<p>one</p><template/><p>two</p>", "one"),
            ("<p>one</p><iframe/><p>two</p>", "one"),
            // HTML inside a graphic belongs to the graphic.
            (
                "<svg><foreignObject><p>inside</p></foreignObject><text>label</text></svg><p>after</p>",
                "after",
            ),
            ("<svg><desc><p>inside</p></desc></svg><p>after</p>", "after"),
            (
                "<math><mtext><b>inside</b></mtext></math><p>after</p>",
                "after",
            ),
            (
                r#"<math><annotation-xml encoding="text/html"><p>inside</p></annotation-xml></math><p>after</p>"#,
                "after",
            ),
            // A graphic cannot end an element that is still open inside it.
            (
                "<svg><foreignObject><template></foreignObject></svg><p>inside</p></template></foreignObject></svg><p>after</p>",
                "after",
            ),
        ] {
            let (rendered, _) = extract_html_sync(html.as_bytes()).unwrap();
            assert_eq!(rendered, expected, "{html}");
        }
    }

    #[test]
    fn html_extractor_starts_a_new_line_where_a_block_ends() {
        for (html, expected) in [
            ("<p>one</p>two", "one\ntwo"),
            ("<h1>Title</h1><span>intro</span> text", "Title\nintro text"),
            (
                "<div><div>one</div></div>two<ul><li>three</li></ul>four",
                "one\ntwo\nthree\nfour",
            ),
            (
                "<table><tr><td>a</td><td>b</td></tr></table>after",
                "a\nb\nafter",
            ),
            (
                r#"<p>For use in examples.</p><a href="https://example.com/more">Learn more</a>"#,
                "For use in examples.\nLearn more [https://example.com/more]",
            ),
            // A browser reads `</br>` as a line break.
            ("one</br>two", "one\ntwo"),
            // Elements inside a line do not separate words.
            (
                "un<em>believ</em>able <b>bold</b>plain",
                "unbelievable boldplain",
            ),
        ] {
            let (rendered, _) = extract_html_sync(html.as_bytes()).unwrap();
            assert_eq!(rendered, expected, "{html}");
        }
    }

    #[test]
    fn html_extractor_separates_what_a_browser_displays_as_a_block() {
        for (html, expected) in [
            (
                r#"<title>Page</title><a href="/home">Home</a>"#,
                "Page\nHome [/home]",
            ),
            (
                "<select><option>one</option><option>two</option></select>after",
                "one\ntwo\nafter",
            ),
            (
                "<details><summary>More</summary>inside</details>after",
                "More\ninside\nafter",
            ),
            (
                "<fieldset><legend>Group</legend>field</fieldset>",
                "Group\nfield",
            ),
            ("<dialog>notice</dialog>after", "notice\nafter"),
        ] {
            let (rendered, _) = extract_html_sync(html.as_bytes()).unwrap();
            assert_eq!(rendered, expected, "{html}");
        }
    }

    #[test]
    fn html_extractor_keeps_a_link_target_with_the_text_of_its_link() {
        for (html, expected) in [
            // A link around blocks.
            (
                r#"<a href="/story"><h3>Title</h3><p>Summary</p></a><p>next</p>"#,
                "Title\nSummary [/story]\nnext",
            ),
            (
                r#"<a href="/story"><div>Title</div></a>next"#,
                "Title [/story]\nnext",
            ),
            (
                r#"<a href="/story"><div>Title</div>more</a>"#,
                "Title\nmore [/story]",
            ),
            (
                r#"<a href="/story">Title<div></div></a>next"#,
                "Title [/story]\nnext",
            ),
            // A link without text does not borrow the line before it.
            (
                r#"<p>one</p><a href="/home"><img src="logo.png"></a><p>two</p>"#,
                "one\n[/home]\ntwo",
            ),
            (
                r#"See <a href="/home"><img src="logo.png"></a> here"#,
                "See [/home] here",
            ),
        ] {
            let (rendered, _) = extract_html_sync(html.as_bytes()).unwrap();
            assert_eq!(rendered, expected, "{html}");
        }
    }
}
