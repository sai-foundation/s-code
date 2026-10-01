use lopdf::{Document, LoadOptions};
use s_code_platform_runtime::{PlatformRuntime, ProcessSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

use crate::web;

const MAX_PDF_BYTES: usize = 8 * 1024 * 1024;
const MAX_DOCUMENT_PAGES: usize = 256;
const MAX_DOCUMENT_OBJECTS: usize = 50_000;
const MAX_PAGES_PER_CALL: u32 = 8;
const MAX_DECOMPRESSED_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 24 * 1024;
const MAX_RESULT_BYTES: usize = 30 * 1024;
const MAX_WORKER_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_ERROR_BYTES: usize = 512;
const WORKER_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PdfReadArgs {
    url: String,
    start_page: u32,
    end_page: u32,
    expected_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PdfReadResult {
    requested_url: String,
    final_url: String,
    status: u16,
    media_type: String,
    page_count: u32,
    start_page: u32,
    end_page: u32,
    content: String,
    body_bytes: u64,
    sha256: String,
    truncated: bool,
    trust: &'static str,
}

#[derive(Debug, Error)]
pub(crate) enum PdfReadError {
    #[error("invalid public PDF request: {0}")]
    InvalidArguments(String),
    #[error("public PDF download failed: {0}")]
    Download(String),
    #[error("public PDF content does not start with a PDF header")]
    InvalidFormat,
    #[error("public PDF changed since the previous page read (SHA-256 mismatch)")]
    HashMismatch,
    #[error("public PDF text extraction failed: {0}")]
    Extraction(String),
    #[error("public PDF result exceeds the inline result limit")]
    ResultTooLarge,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerResponse {
    Completed {
        page_count: u32,
        content: String,
        truncated: bool,
        sha256: String,
    },
    Failed {
        error: String,
    },
}

pub(crate) fn parse_arguments(value: &Value) -> Result<PdfReadArgs, PdfReadError> {
    let mut input: PdfReadArgs = serde_json::from_value(value.clone())
        .map_err(|error| PdfReadError::InvalidArguments(bounded_error(error)))?;
    input.url = web::normalize_public_url(&input.url)
        .map_err(|error| PdfReadError::InvalidArguments(error.to_string()))?;
    if input.start_page == 0 || input.end_page < input.start_page {
        return Err(PdfReadError::InvalidArguments(
            "start_page and end_page must form a 1-based inclusive range".into(),
        ));
    }
    let page_span = input.end_page - input.start_page + 1;
    if page_span > MAX_PAGES_PER_CALL {
        return Err(PdfReadError::InvalidArguments(format!(
            "a call can read at most {MAX_PAGES_PER_CALL} consecutive pages"
        )));
    }
    if input.end_page as usize > MAX_DOCUMENT_PAGES {
        return Err(PdfReadError::InvalidArguments(format!(
            "page numbers cannot exceed {MAX_DOCUMENT_PAGES}"
        )));
    }
    if let Some(expected) = &mut input.expected_sha256 {
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PdfReadError::InvalidArguments(
                "expected_sha256 must contain exactly 64 hexadecimal characters".into(),
            ));
        }
        expected.make_ascii_lowercase();
    }
    Ok(input)
}

pub(crate) fn normalized_arguments(value: &Value) -> Result<Value, PdfReadError> {
    let input = parse_arguments(value)?;
    let mut normalized = serde_json::json!({
        "url": input.url,
        "start_page": input.start_page,
        "end_page": input.end_page,
    });
    if let Some(expected_sha256) = input.expected_sha256 {
        normalized["expected_sha256"] = Value::String(expected_sha256);
    }
    Ok(normalized)
}

pub(crate) async fn read(
    input: PdfReadArgs,
    platform: Arc<dyn PlatformRuntime>,
) -> Result<PdfReadResult, PdfReadError> {
    let downloaded = web::download_pdf(&input.url)
        .await
        .map_err(|error| PdfReadError::Download(error.to_string()))?;
    if !downloaded.bytes.starts_with(b"%PDF-") {
        return Err(PdfReadError::InvalidFormat);
    }
    let sha256 = format!("{:x}", Sha256::digest(&downloaded.bytes));
    if input
        .expected_sha256
        .as_ref()
        .is_some_and(|expected| expected != &sha256)
    {
        return Err(PdfReadError::HashMismatch);
    }
    let extraction = run_worker(
        platform,
        &downloaded.bytes,
        input.start_page,
        input.end_page,
        &sha256,
    )
    .await?;
    let WorkerResponse::Completed {
        page_count,
        content,
        truncated,
        sha256: parsed_sha256,
    } = extraction
    else {
        unreachable!("run_worker converts failed worker responses")
    };
    if parsed_sha256 != sha256 {
        return Err(PdfReadError::Extraction(
            "isolated parser input changed before it was read".into(),
        ));
    }
    bounded_result(PdfReadResult {
        requested_url: downloaded.requested_url,
        final_url: downloaded.final_url,
        status: downloaded.status,
        media_type: downloaded.media_type,
        page_count,
        start_page: input.start_page,
        end_page: input.end_page,
        content,
        body_bytes: downloaded.bytes.len() as u64,
        sha256,
        truncated,
        trust: "remote_untrusted",
    })
}

async fn run_worker(
    platform: Arc<dyn PlatformRuntime>,
    bytes: &[u8],
    start_page: u32,
    end_page: u32,
    expected_sha256: &str,
) -> Result<WorkerResponse, PdfReadError> {
    let directory = tempfile::Builder::new()
        .prefix("s-code-pdf.")
        .tempdir()
        .map_err(extraction_error)?;
    let document_path = directory.path().join("document.pdf");
    let mut document = tempfile::Builder::new()
        .prefix("input.")
        .suffix(".pdf")
        .tempfile_in(directory.path())
        .map_err(extraction_error)?;
    document.write_all(bytes).map_err(extraction_error)?;
    document.flush().map_err(extraction_error)?;
    document
        .persist(&document_path)
        .map_err(|error| extraction_error(error.error))?;

    let current_executable = std::env::current_exe()
        .map_err(extraction_error)?
        .canonicalize()
        .map_err(extraction_error)?;
    let executable_directory = current_executable
        .parent()
        .ok_or_else(|| PdfReadError::Extraction("parser executable has no parent".into()))?;
    let executable = executable_directory.join(if cfg!(windows) {
        "s-code-pdf-worker.exe"
    } else {
        "s-code-pdf-worker"
    });
    let executable = executable.canonicalize().map_err(|_| {
        PdfReadError::Extraction("isolated PDF parser is not installed beside the daemon".into())
    })?;
    let directory_uri = url::Url::from_directory_path(directory.path())
        .map_err(|_| PdfReadError::Extraction("parser directory is not absolute".into()))?
        .to_string();
    let executable_uri = url::Url::from_directory_path(executable_directory)
        .map_err(|_| PdfReadError::Extraction("parser executable path is not absolute".into()))?
        .to_string();
    let output = platform
        .execute(ProcessSpec {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                document_path.to_string_lossy().into_owned(),
                start_page.to_string(),
                end_page.to_string(),
                expected_sha256.to_owned(),
            ],
            cwd_uri: directory_uri.clone(),
            environment_handles: BTreeMap::new(),
            timeout: WORKER_TIMEOUT,
            network_enabled: false,
            browser_compatible: false,
            readable_root_uris: vec![directory_uri, executable_uri],
            writable_root_uris: Vec::new(),
            denied_read_uris: Vec::new(),
            output_limit_bytes: MAX_WORKER_OUTPUT_BYTES,
            #[cfg(unix)]
            pinned_cwd: Some(Arc::new(
                File::open(directory.path()).map_err(extraction_error)?,
            )),
        })
        .await
        .map_err(|error| {
            let detail = error.to_string();
            if detail.contains("timed out") {
                PdfReadError::Extraction("parser exceeded the 12 second limit".into())
            } else {
                PdfReadError::Extraction(bounded_error(detail))
            }
        })?;
    if output.truncated {
        return Err(PdfReadError::Extraction(
            "parser response exceeded its output limit".into(),
        ));
    }
    if output.exit_code != Some(0) {
        return Err(PdfReadError::Extraction(
            "isolated parser stopped before returning a result".into(),
        ));
    }
    let response: WorkerResponse = serde_json::from_str(&output.stdout).map_err(|_| {
        PdfReadError::Extraction("isolated parser returned an invalid result".into())
    })?;
    match response {
        WorkerResponse::Failed { error } => Err(PdfReadError::Extraction(bounded_error(error))),
        completed => Ok(completed),
    }
}

pub(super) fn worker(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let response = catch_unwind(AssertUnwindSafe(|| worker_inner(arguments)))
        .unwrap_or_else(|_| Err("parser stopped safely after malformed PDF input".into()))
        .unwrap_or_else(|error| WorkerResponse::Failed {
            error: bounded_error(error),
        });
    serde_json::to_writer(std::io::stdout().lock(), &response)?;
    println!();
    Ok(())
}

fn worker_inner(arguments: &[String]) -> Result<WorkerResponse, String> {
    apply_worker_limits()?;
    if arguments.len() != 4 {
        return Err("invalid isolated parser arguments".into());
    }
    let start_page = arguments[1]
        .parse::<u32>()
        .map_err(|_| "invalid start page")?;
    let end_page = arguments[2]
        .parse::<u32>()
        .map_err(|_| "invalid end page")?;
    let expected_sha256 = &arguments[3];
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid isolated parser digest".into());
    }
    if start_page == 0 || end_page < start_page || end_page - start_page + 1 > MAX_PAGES_PER_CALL {
        return Err("invalid isolated parser page range".into());
    }
    let path = Path::new(&arguments[0]);
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "PDF input is unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("PDF input must be a regular file".into());
    }
    if metadata.len() > MAX_PDF_BYTES as u64 {
        return Err("PDF input exceeds the 8 MiB limit".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|_| "PDF input is unavailable")?
        .take((MAX_PDF_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "PDF input could not be read")?;
    if bytes.len() > MAX_PDF_BYTES {
        return Err("PDF input exceeds the 8 MiB limit".into());
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err("PDF input changed before isolated parsing".into());
    }
    extract_text(&bytes, start_page, end_page)
}

fn extract_text(bytes: &[u8], start_page: u32, end_page: u32) -> Result<WorkerResponse, String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err("content is not a PDF".into());
    }
    let document = Document::load_mem_with_options(
        bytes,
        LoadOptions {
            strict: true,
            max_decompressed_size: Some(MAX_DECOMPRESSED_STREAM_BYTES),
            ..Default::default()
        },
    )
    .map_err(|error| match error {
        lopdf::Error::InvalidPassword => "encrypted PDFs are not supported".to_owned(),
        _ => "PDF structure is invalid or unsupported".to_owned(),
    })?;
    if document.is_encrypted() || document.was_encrypted() {
        return Err("encrypted PDFs are not supported".into());
    }
    if document.objects.len() > MAX_DOCUMENT_OBJECTS {
        return Err(format!(
            "PDF contains more than {MAX_DOCUMENT_OBJECTS} objects"
        ));
    }
    let page_count = document.page_iter().take(MAX_DOCUMENT_PAGES + 1).count();
    if page_count == 0 {
        return Err("PDF contains no pages".into());
    }
    if page_count > MAX_DOCUMENT_PAGES {
        return Err(format!("PDF contains more than {MAX_DOCUMENT_PAGES} pages"));
    }
    let page_count = page_count as u32;
    if end_page > page_count {
        return Err(format!(
            "requested page {end_page}, but the PDF has {page_count} pages"
        ));
    }

    let mut content = String::new();
    let mut truncated = false;
    for page in start_page..=end_page {
        let separator = if content.is_empty() { "" } else { "\n" };
        let header = format!("{separator}--- Page {page} ---\n");
        if !push_bounded(&mut content, &header, MAX_TEXT_BYTES) {
            truncated = true;
            break;
        }
        let extracted = document
            .extract_text_with_limit(&[page], MAX_DECOMPRESSED_STREAM_BYTES)
            .map_err(|_| format!("page {page} text is invalid, unsupported, or too large"))?;
        let sanitized = sanitize_text(&extracted);
        let page_text = if sanitized.trim().is_empty() {
            "[No extractable text; this page may be scanned or visual.]\n"
        } else {
            sanitized.as_str()
        };
        if !push_bounded(&mut content, page_text, MAX_TEXT_BYTES) {
            truncated = true;
            break;
        }
        if !content.ends_with('\n') && content.len() < MAX_TEXT_BYTES {
            content.push('\n');
        }
    }
    Ok(WorkerResponse::Completed {
        page_count,
        content,
        truncated,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn sanitize_text(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len().min(MAX_TEXT_BYTES));
    let mut previous_was_cr = false;
    for character in value.chars() {
        match character {
            '\r' => {
                sanitized.push('\n');
                previous_was_cr = true;
            }
            '\n' if previous_was_cr => previous_was_cr = false,
            '\n' => {
                sanitized.push('\n');
                previous_was_cr = false;
            }
            '\t' => {
                sanitized.push(' ');
                previous_was_cr = false;
            }
            value if value.is_control() => previous_was_cr = false,
            value => {
                sanitized.push(value);
                previous_was_cr = false;
            }
        }
    }
    sanitized
}

fn push_bounded(output: &mut String, value: &str, maximum: usize) -> bool {
    let remaining = maximum.saturating_sub(output.len());
    if value.len() <= remaining {
        output.push_str(value);
        return true;
    }
    let mut end = remaining.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&value[..end]);
    false
}

fn bounded_result(mut result: PdfReadResult) -> Result<PdfReadResult, PdfReadError> {
    while serde_json::to_vec(&result)
        .map_err(|error| PdfReadError::Extraction(bounded_error(error)))?
        .len()
        > MAX_RESULT_BYTES
    {
        if result.content.is_empty() {
            return Err(PdfReadError::ResultTooLarge);
        }
        let target = result.content.len().saturating_sub(1024);
        let mut end = target;
        while end > 0 && !result.content.is_char_boundary(end) {
            end -= 1;
        }
        result.content.truncate(end);
        result.truncated = true;
    }
    Ok(result)
}

fn extraction_error(error: impl std::fmt::Display) -> PdfReadError {
    PdfReadError::Extraction(bounded_error(error))
}

fn bounded_error(value: impl std::fmt::Display) -> String {
    let value = value.to_string();
    let mut end = value.len().min(MAX_ERROR_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(unix)]
fn apply_worker_limits() -> Result<(), String> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

    let cap = |resource, requested| {
        let inherited = getrlimit(resource);
        let maximum = inherited
            .maximum
            .map_or(requested, |value| value.min(requested));
        let current = inherited
            .current
            .map_or(requested, |value| value.min(requested))
            .min(maximum);
        setrlimit(
            resource,
            Rlimit {
                current: Some(current),
                maximum: Some(maximum),
            },
        )
    };

    for (resource, value) in [
        (Resource::Cpu, 8),
        (Resource::Core, 0),
        (Resource::Fsize, 0),
        (Resource::Nofile, 32),
    ] {
        cap(resource, value).map_err(|_| "isolated parser resource limits are unavailable")?;
    }
    #[cfg(target_os = "linux")]
    cap(Resource::As, 512 * 1024 * 1024)
        .map_err(|_| "isolated parser memory limit is unavailable")?;
    Ok(())
}

#[cfg(not(unix))]
fn apply_worker_limits() -> Result<(), String> {
    Err("isolated PDF parsing is unavailable on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{Object, Stream, content::Content, content::Operation, dictionary};

    fn text_pdf(page_text: &[&str]) -> Vec<u8> {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = document.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut page_ids = Vec::new();
        for text in page_text {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new("Tj", vec![Object::string_literal(*text)]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = document.add_object(Stream::new(
                dictionary! {},
                content.encode().expect("encode content"),
            ));
            let page_id = document.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            });
            page_ids.push(page_id);
        }
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().copied().map(Object::Reference).collect::<Vec<_>>(),
                "Count" => page_ids.len() as i64,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("save PDF");
        bytes
    }

    #[test]
    fn arguments_are_normalized_and_bounded() {
        let normalized = normalized_arguments(&serde_json::json!({
            "url":"https://EXAMPLE.com:443/paper.pdf#page=2",
            "start_page":2,
            "end_page":3,
            "expected_sha256":"AA00000000000000000000000000000000000000000000000000000000000000"
        }))
        .unwrap();
        assert_eq!(normalized["url"], "https://example.com/paper.pdf");
        assert_eq!(
            normalized["expected_sha256"],
            "aa00000000000000000000000000000000000000000000000000000000000000"
        );
        assert!(
            parse_arguments(&serde_json::json!({
                "url":"https://example.com/paper.pdf",
                "start_page":1,
                "end_page":9
            }))
            .is_err()
        );
    }

    #[test]
    fn extraction_preserves_page_boundaries_and_selected_range() {
        let bytes = text_pdf(&["first", "second", "third"]);
        let WorkerResponse::Completed {
            page_count,
            content,
            truncated,
            sha256,
        } = extract_text(&bytes, 2, 3).unwrap()
        else {
            panic!("expected completed extraction")
        };
        assert_eq!(page_count, 3);
        assert!(!content.contains("first"));
        assert!(content.contains("--- Page 2 ---\nsecond"));
        assert!(content.contains("--- Page 3 ---\nthird"));
        assert!(!truncated);
        assert_eq!(sha256, format!("{:x}", Sha256::digest(&bytes)));
    }

    #[test]
    fn extraction_rejects_non_pdf_and_out_of_range_pages() {
        assert!(extract_text(b"not a pdf", 1, 1).is_err());
        let bytes = text_pdf(&["only"]);
        assert!(
            extract_text(&bytes, 1, 2)
                .unwrap_err()
                .contains("has 1 pages")
        );
    }

    #[test]
    fn sanitation_removes_terminal_controls_without_losing_lines() {
        assert_eq!(sanitize_text("a\r\nb\t\u{1b}[31m"), "a\nb [31m");
    }
}
