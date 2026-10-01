use hayro::{
    RenderSettings,
    hayro_interpret::{
        BlendMode, CacheKey, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterSettings,
        PageExt, Paint, PathDrawMode, SoftMask, font::Glyph, interpret_page, pattern::Pattern,
    },
    hayro_syntax::{
        Pdf,
        object::{
            Array, Dict,
            dict::keys::{ANNOTS, AP, F},
        },
    },
    render,
    vello_cpu::{
        Pixmap,
        color::palette::css::WHITE,
        kurbo::{Affine, BezPath, Point, Rect},
    },
};
use lopdf::{Document, LoadOptions};
use s_code_platform_runtime::{PlatformRuntime, ProcessSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use thiserror::Error;

use crate::web;

const MAX_DOCUMENT_PAGES: usize = 256;
const MAX_DOCUMENT_OBJECTS: usize = 50_000;
const MAX_PAGES_PER_CALL: u32 = 8;
const MAX_DECOMPRESSED_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_TEXT_BYTES: usize = 24 * 1024;
const MAX_RESULT_BYTES: usize = 30 * 1024;
const MAX_WORKER_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_RENDERED_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_RENDER_LONG_EDGE: u16 = 1_200;
const MAX_RENDER_PIXELS: u32 = 1_200_000;
const MAX_ERROR_BYTES: usize = 512;
const WORKER_TIMEOUT: Duration = Duration::from_secs(12);
static PDF_WORKER_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static PDF_RENDER_LOCK: StdMutex<()> = StdMutex::new(());
static HAYRO_RENDER_WARNING: AtomicBool = AtomicBool::new(false);
static HAYRO_LOGGER_READY: OnceLock<bool> = OnceLock::new();

struct HayroWarningLogger;

static HAYRO_WARNING_LOGGER: HayroWarningLogger = HayroWarningLogger;

impl log::Log for HayroWarningLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("hayro_")
            && matches!(metadata.level(), log::Level::Error | log::Level::Warn)
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            HAYRO_RENDER_WARNING.store(true, Ordering::Release);
        }
    }

    fn flush(&self) {}
}

/// Hayro 0.5 omits some undecodable images without reporting them. This device
/// draws nothing: it replays the page and records any image whose pixels are
/// unavailable, including images inside Type 3 glyphs, tiling patterns and
/// soft masks.
#[derive(Default)]
struct ImageAudit {
    nested: HashSet<u128>,
    undecodable_image: bool,
}

impl ImageAudit {
    fn paint<'a>(&mut self, paint: &Paint<'a>, is_stroke: bool) {
        if let Paint::Pattern(pattern) = paint
            && let Pattern::Tiling(tiling) = pattern.as_ref()
            && self.nested.insert(tiling.cache_key())
        {
            tiling.interpret(self, Affine::IDENTITY, is_stroke);
        }
    }

    fn soft_mask<'a>(&mut self, mask: Option<SoftMask<'a>>) {
        if let Some(mask) = mask
            && self.nested.insert(mask.cache_key())
        {
            mask.interpret(self);
        }
    }
}

impl<'a> Device<'a> for ImageAudit {
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        self.soft_mask(mask);
    }

    fn set_blend_mode(&mut self, _: BlendMode) {}

    fn draw_path(&mut self, _: &BezPath, _: Affine, paint: &Paint<'a>, mode: &PathDrawMode) {
        self.paint(paint, matches!(mode, PathDrawMode::Stroke(_)));
    }

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, mask: Option<SoftMask<'a>>, _: BlendMode) {
        self.soft_mask(mask);
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &Paint<'a>,
        mode: &GlyphDrawMode,
    ) {
        if matches!(mode, GlyphDrawMode::Invisible) {
            return;
        }
        match glyph {
            Glyph::Outline(_) => self.paint(paint, matches!(mode, GlyphDrawMode::Stroke(_))),
            Glyph::Type3(glyph) => glyph.interpret(self, transform, glyph_transform, paint),
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        // Request the renderer's resolution so both passes take one decode path.
        let edge = |x, y| (transform * Point::new(x, y)).to_vec2().length().ceil() as u32;
        let resolution = Some((
            edge(f64::from(image.width()), 0.0),
            edge(0.0, f64::from(image.height())),
        ));
        let mut decoded = false;
        match image {
            Image::Stencil(stencil) => stencil.with_stencil(|_, _| decoded = true, resolution),
            Image::Raster(raster) => raster.with_rgba(|_, _| decoded = true, resolution),
        }
        self.undecodable_image |= !decoded;
    }

    fn pop_clip_path(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PdfViewArgs {
    url: String,
    page: u32,
    expected_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PdfViewResult {
    pub(crate) requested_url: String,
    pub(crate) final_url: String,
    pub(crate) status: u16,
    pub(crate) source_media_type: String,
    pub(crate) page_count: u32,
    pub(crate) page: u32,
    pub(crate) body_bytes: u64,
    pub(crate) sha256: String,
    pub(crate) image_media_type: String,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) image_bytes: u64,
    pub(crate) image_sha256: String,
    /// Form fields and annotations that could not be drawn and are therefore
    /// absent from the image, which then shows the page's own content only.
    pub(crate) annotations_omitted: u32,
    pub(crate) trust: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct PdfPageImage {
    pub(crate) media_type: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct PdfViewOutput {
    pub(crate) result: PdfViewResult,
    pub(crate) image: PdfPageImage,
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
    #[error("rendered PDF page exceeds the 5 MiB image limit")]
    ImageTooLarge,
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
    Rendered {
        page_count: u32,
        page: u32,
        image_width: u32,
        image_height: u32,
        image_bytes: u64,
        image_sha256: String,
        annotations_omitted: u32,
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
    normalize_expected_sha256(&mut input.expected_sha256)?;
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

pub(crate) fn parse_view_arguments(value: &Value) -> Result<PdfViewArgs, PdfReadError> {
    let mut input: PdfViewArgs = serde_json::from_value(value.clone())
        .map_err(|error| PdfReadError::InvalidArguments(bounded_error(error)))?;
    input.url = web::normalize_public_url(&input.url)
        .map_err(|error| PdfReadError::InvalidArguments(error.to_string()))?;
    if input.page == 0 || input.page as usize > MAX_DOCUMENT_PAGES {
        return Err(PdfReadError::InvalidArguments(format!(
            "page must be between 1 and {MAX_DOCUMENT_PAGES}"
        )));
    }
    normalize_expected_sha256(&mut input.expected_sha256)?;
    Ok(input)
}

pub(crate) fn normalized_view_arguments(value: &Value) -> Result<Value, PdfReadError> {
    let input = parse_view_arguments(value)?;
    let mut normalized = serde_json::json!({
        "url": input.url,
        "page": input.page,
    });
    if let Some(expected_sha256) = input.expected_sha256 {
        normalized["expected_sha256"] = Value::String(expected_sha256);
    }
    Ok(normalized)
}

fn normalize_expected_sha256(value: &mut Option<String>) -> Result<(), PdfReadError> {
    if let Some(expected) = value {
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PdfReadError::InvalidArguments(
                "expected_sha256 must contain exactly 64 hexadecimal characters".into(),
            ));
        }
        expected.make_ascii_lowercase();
    }
    Ok(())
}

pub(crate) async fn read(
    input: PdfReadArgs,
    platform: Arc<dyn PlatformRuntime>,
) -> Result<PdfReadResult, PdfReadError> {
    let downloaded = web::download_pdf(&input.url, "pdf_read")
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

pub(crate) async fn view(
    input: PdfViewArgs,
    platform: Arc<dyn PlatformRuntime>,
) -> Result<PdfViewOutput, PdfReadError> {
    let downloaded = web::download_pdf(&input.url, "pdf_view")
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

    let rendered = run_view_worker(platform, &downloaded.bytes, input.page, &sha256).await?;
    let WorkerResponse::Rendered {
        page_count,
        page,
        image_width,
        image_height,
        image_bytes,
        image_sha256,
        annotations_omitted,
        sha256: parsed_sha256,
    } = rendered.response
    else {
        unreachable!("run_view_worker accepts only rendered worker responses")
    };
    if parsed_sha256 != sha256 {
        return Err(PdfReadError::Extraction(
            "isolated renderer input changed before it was read".into(),
        ));
    }

    let image_media_type = "image/png".to_owned();
    Ok(PdfViewOutput {
        result: PdfViewResult {
            requested_url: downloaded.requested_url,
            final_url: downloaded.final_url,
            status: downloaded.status,
            source_media_type: downloaded.media_type,
            page_count,
            page,
            body_bytes: downloaded.bytes.len() as u64,
            sha256,
            image_media_type: image_media_type.clone(),
            image_width,
            image_height,
            image_bytes,
            image_sha256,
            annotations_omitted,
            trust: "remote_untrusted",
        },
        image: PdfPageImage {
            media_type: image_media_type,
            bytes: rendered.image,
        },
    })
}

struct WorkerViewOutput {
    response: WorkerResponse,
    image: Vec<u8>,
}

async fn run_view_worker(
    platform: Arc<dyn PlatformRuntime>,
    bytes: &[u8],
    page: u32,
    expected_sha256: &str,
) -> Result<WorkerViewOutput, PdfReadError> {
    let _worker_permit = PDF_WORKER_LIMIT.acquire().await.map_err(|_| {
        PdfReadError::Extraction("isolated PDF worker capacity is unavailable".into())
    })?;
    let directory = tempfile::Builder::new()
        .prefix("s-code-pdf.")
        .tempdir()
        .map_err(extraction_error)?;
    let input_directory = directory.path().join("input");
    let output_directory = directory.path().join("output");
    std::fs::create_dir(&input_directory).map_err(extraction_error)?;
    std::fs::create_dir(&output_directory).map_err(extraction_error)?;

    let document_path = input_directory.join("document.pdf");
    let mut document = tempfile::Builder::new()
        .prefix("input.")
        .suffix(".pdf")
        .tempfile_in(&input_directory)
        .map_err(extraction_error)?;
    document.write_all(bytes).map_err(extraction_error)?;
    document.flush().map_err(extraction_error)?;
    document
        .persist(&document_path)
        .map_err(|error| extraction_error(error.error))?;
    let image_path = output_directory.join("page.png");

    let current_executable = std::env::current_exe()
        .map_err(extraction_error)?
        .canonicalize()
        .map_err(extraction_error)?;
    let executable_directory = current_executable
        .parent()
        .ok_or_else(|| PdfReadError::Extraction("renderer executable has no parent".into()))?;
    let executable = executable_directory.join(if cfg!(windows) {
        "s-code-pdf-worker.exe"
    } else {
        "s-code-pdf-worker"
    });
    let executable = executable.canonicalize().map_err(|_| {
        PdfReadError::Extraction("isolated PDF renderer is not installed beside the daemon".into())
    })?;
    let input_uri = url::Url::from_directory_path(&input_directory)
        .map_err(|_| PdfReadError::Extraction("renderer input is not absolute".into()))?
        .to_string();
    let output_uri = url::Url::from_directory_path(&output_directory)
        .map_err(|_| PdfReadError::Extraction("renderer output is not absolute".into()))?
        .to_string();
    let executable_uri = url::Url::from_directory_path(executable_directory)
        .map_err(|_| PdfReadError::Extraction("renderer executable path is not absolute".into()))?
        .to_string();
    let output = platform
        .execute(ProcessSpec {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "view".into(),
                document_path.to_string_lossy().into_owned(),
                page.to_string(),
                expected_sha256.to_owned(),
                image_path.to_string_lossy().into_owned(),
            ],
            cwd_uri: output_uri.clone(),
            environment_handles: BTreeMap::new(),
            timeout: WORKER_TIMEOUT,
            network_enabled: false,
            browser_compatible: false,
            readable_root_uris: vec![input_uri, executable_uri],
            writable_root_uris: vec![output_uri],
            denied_read_uris: Vec::new(),
            output_limit_bytes: MAX_WORKER_OUTPUT_BYTES,
            #[cfg(unix)]
            pinned_cwd: Some(Arc::new(
                File::open(&output_directory).map_err(extraction_error)?,
            )),
        })
        .await
        .map_err(|error| {
            let detail = error.to_string();
            if detail.contains("timed out") {
                PdfReadError::Extraction("renderer exceeded the 12 second limit".into())
            } else {
                PdfReadError::Extraction(bounded_error(detail))
            }
        })?;
    if output.truncated {
        return Err(PdfReadError::Extraction(
            "renderer response exceeded its output limit".into(),
        ));
    }
    if output.exit_code != Some(0) {
        return Err(PdfReadError::Extraction(
            "isolated renderer stopped before returning a result".into(),
        ));
    }
    let response: WorkerResponse = serde_json::from_str(&output.stdout).map_err(|_| {
        PdfReadError::Extraction("isolated renderer returned an invalid result".into())
    })?;
    match &response {
        WorkerResponse::Failed { error } => {
            return Err(PdfReadError::Extraction(bounded_error(error)));
        }
        WorkerResponse::Rendered {
            page: rendered_page,
            image_width,
            image_height,
            image_bytes,
            image_sha256,
            ..
        } => {
            if *rendered_page != page {
                return Err(PdfReadError::Extraction(
                    "isolated renderer returned the wrong page".into(),
                ));
            }
            let metadata = std::fs::symlink_metadata(&image_path)
                .map_err(|_| PdfReadError::Extraction("renderer image is unavailable".into()))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(PdfReadError::Extraction(
                    "renderer image must be a regular file".into(),
                ));
            }
            if metadata.len() > MAX_RENDERED_IMAGE_BYTES as u64 {
                return Err(PdfReadError::ImageTooLarge);
            }
            if metadata.len() != *image_bytes {
                return Err(PdfReadError::Extraction(
                    "renderer image size did not match its metadata".into(),
                ));
            }
            let mut image = Vec::with_capacity(metadata.len() as usize);
            File::open(&image_path)
                .map_err(|_| PdfReadError::Extraction("renderer image is unavailable".into()))?
                .take((MAX_RENDERED_IMAGE_BYTES + 1) as u64)
                .read_to_end(&mut image)
                .map_err(extraction_error)?;
            if image.len() > MAX_RENDERED_IMAGE_BYTES {
                return Err(PdfReadError::ImageTooLarge);
            }
            validate_png(&image, *image_width, *image_height)?;
            if format!("{:x}", Sha256::digest(&image)) != *image_sha256 {
                return Err(PdfReadError::Extraction(
                    "renderer image digest did not match its metadata".into(),
                ));
            }
            return Ok(WorkerViewOutput { response, image });
        }
        WorkerResponse::Completed { .. } => {}
    }
    Err(PdfReadError::Extraction(
        "isolated renderer returned a text result".into(),
    ))
}

fn validate_png(
    bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
) -> Result<(), PdfReadError> {
    let valid_header = bytes.len() >= 24
        && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        && bytes[8..12] == 13_u32.to_be_bytes()
        && &bytes[12..16] == b"IHDR";
    if !valid_header {
        return Err(PdfReadError::Extraction(
            "renderer output is not a valid PNG".into(),
        ));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().expect("four PNG width bytes"));
    let height = u32::from_be_bytes(bytes[20..24].try_into().expect("four PNG height bytes"));
    if width != expected_width
        || height != expected_height
        || width == 0
        || height == 0
        || width > u32::from(MAX_RENDER_LONG_EDGE)
        || height > u32::from(MAX_RENDER_LONG_EDGE)
        || width.saturating_mul(height) > MAX_RENDER_PIXELS
    {
        return Err(PdfReadError::Extraction(
            "renderer image dimensions are invalid".into(),
        ));
    }
    Ok(())
}

async fn run_worker(
    platform: Arc<dyn PlatformRuntime>,
    bytes: &[u8],
    start_page: u32,
    end_page: u32,
    expected_sha256: &str,
) -> Result<WorkerResponse, PdfReadError> {
    let _worker_permit = PDF_WORKER_LIMIT.acquire().await.map_err(|_| {
        PdfReadError::Extraction("isolated PDF worker capacity is unavailable".into())
    })?;
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
                "text".into(),
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
    let Some((mode, arguments)) = arguments.split_first() else {
        return Err("invalid isolated PDF worker arguments".into());
    };
    match mode.as_str() {
        "text" => {
            apply_worker_limits(0)?;
            text_worker_inner(arguments)
        }
        "view" => {
            apply_worker_limits(MAX_RENDERED_IMAGE_BYTES as u64)?;
            view_worker_inner(arguments)
        }
        _ => Err("invalid isolated PDF worker mode".into()),
    }
}

fn text_worker_inner(arguments: &[String]) -> Result<WorkerResponse, String> {
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
    let bytes = read_worker_input(Path::new(&arguments[0]), expected_sha256)?;
    extract_text(&bytes, start_page, end_page)
}

fn view_worker_inner(arguments: &[String]) -> Result<WorkerResponse, String> {
    if arguments.len() != 4 {
        return Err("invalid isolated renderer arguments".into());
    }
    let page = arguments[1]
        .parse::<u32>()
        .map_err(|_| "invalid page number")?;
    if page == 0 || page as usize > MAX_DOCUMENT_PAGES {
        return Err("invalid isolated renderer page number".into());
    }
    let expected_sha256 = &arguments[2];
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid isolated renderer digest".into());
    }
    let bytes = read_worker_input(Path::new(&arguments[0]), expected_sha256)?;
    render_page(&bytes, page, Path::new(&arguments[3]))
}

fn read_worker_input(path: &Path, expected_sha256: &str) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "PDF input is unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("PDF input must be a regular file".into());
    }
    if metadata.len() > web::MAX_PDF_BODY_BYTES as u64 {
        return Err("PDF input exceeds the 16 MiB limit".into());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|_| "PDF input is unavailable")?
        .take((web::MAX_PDF_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "PDF input could not be read")?;
    if bytes.len() > web::MAX_PDF_BODY_BYTES {
        return Err("PDF input exceeds the 16 MiB limit".into());
    }
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        return Err("PDF input changed before isolated parsing".into());
    }
    Ok(bytes)
}

fn extract_text(bytes: &[u8], start_page: u32, end_page: u32) -> Result<WorkerResponse, String> {
    let document = load_validated_document(bytes)?;
    let page_count = validated_page_count(&document)?;
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

/// One rendering attempt. Every attempt parses the PDF afresh so a warning
/// Hayro reports only once cannot be hidden by an earlier attempt.
struct PageRender {
    pixmap: Pixmap,
    image_width: u16,
    image_height: u16,
    /// Annotations that carry an appearance and are not hidden.
    annotations: u32,
}

/// Hayro 0.5 omits content it cannot decode and at most logs the failure. The
/// isolated worker turns those warnings, and any image the audit finds without
/// pixels, into no result. The warning sink stays so a later compatible Hayro
/// release cannot regress to silently incomplete pixels.
fn render_faithfully(
    bytes: &Arc<Vec<u8>>,
    page: u32,
    page_count: u32,
    render_annotations: bool,
) -> Result<Option<PageRender>, String> {
    HAYRO_RENDER_WARNING.store(false, Ordering::Release);
    let callback_warning = Arc::new(AtomicBool::new(false));
    let warned = {
        let callback_warning = callback_warning.clone();
        move || {
            callback_warning.load(Ordering::Acquire) || HAYRO_RENDER_WARNING.load(Ordering::Acquire)
        }
    };
    let mut settings = InterpreterSettings {
        render_annotations,
        ..Default::default()
    };
    settings.warning_sink = Arc::new(move |_| {
        callback_warning.store(true, Ordering::Release);
    });

    let pdf =
        Pdf::new(bytes.clone()).map_err(|_| "PDF structure is unsupported by the renderer")?;
    if pdf.pages().len() != page_count as usize {
        return Err("PDF page structure is unsupported by the renderer".into());
    }
    let page = &pdf.pages()[(page - 1) as usize];
    let (page_width, page_height) = page.render_dimensions();
    let (scale, image_width, image_height) = bounded_render_dimensions(page_width, page_height)?;
    let mut audit = ImageAudit::default();
    interpret_page(
        page,
        &mut Context::new(
            Affine::scale(f64::from(scale)) * page.initial_transform(true),
            Rect::new(0.0, 0.0, f64::from(image_width), f64::from(image_height)),
            page.xref(),
            settings.clone(),
        ),
        &mut audit,
    );
    if audit.undecodable_image || warned() {
        return Ok(None);
    }
    let pixmap = render(
        page,
        &settings,
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(image_width),
            height: Some(image_height),
            bg_color: WHITE,
        },
    );
    if warned() {
        return Ok(None);
    }
    // Counted only after the verdict: reading an annotation Hayro cannot parse
    // must not count against the page's own content. An unreadable annotation
    // is assumed to be visible.
    let annotations = page
        .raw()
        .get::<Array<'_>>(ANNOTS)
        .map_or(0, |annotations| {
            let invisible = annotations
                .iter::<Dict<'_>>()
                .filter(|annotation| {
                    annotation.get::<u32>(F).unwrap_or(0) & 2 != 0 || !annotation.contains_key(AP)
                })
                .count();
            annotations.raw_iter().count().saturating_sub(invisible)
        });
    Ok(Some(PageRender {
        pixmap,
        image_width,
        image_height,
        annotations: u32::try_from(annotations).unwrap_or(u32::MAX),
    }))
}

fn render_page(bytes: &[u8], page: u32, output_path: &Path) -> Result<WorkerResponse, String> {
    let _render_guard = PDF_RENDER_LOCK
        .lock()
        .map_err(|_| "PDF renderer warning monitor is unavailable")?;
    let logger_ready =
        HAYRO_LOGGER_READY.get_or_init(|| log::set_logger(&HAYRO_WARNING_LOGGER).is_ok());
    if !*logger_ready {
        return Err("PDF renderer warning monitor is unavailable".into());
    }
    log::set_max_level(log::LevelFilter::Warn);

    let document = load_validated_document(bytes)?;
    let page_count = validated_page_count(&document)?;
    if page > page_count {
        return Err(format!(
            "requested page {page}, but the PDF has {page_count} pages"
        ));
    }
    drop(document);

    // The page's own content is judged alone first, so annotations Hayro cannot
    // draw never excuse missing content. Such annotations are then left out
    // together and counted instead of failing a page that is otherwise whole.
    let renderer_input = Arc::new(bytes.to_vec());
    let content = render_faithfully(&renderer_input, page, page_count, false)?
        .ok_or("PDF page could not be rendered faithfully")?;
    let (rendered, annotations_omitted) = if content.annotations == 0 {
        (content, 0)
    } else {
        match render_faithfully(&renderer_input, page, page_count, true)? {
            Some(complete) => (complete, 0),
            None => {
                let omitted = content.annotations;
                (content, omitted)
            }
        }
    };
    let PageRender {
        pixmap,
        image_width,
        image_height,
        ..
    } = rendered;
    let image = pixmap
        .into_png()
        .map_err(|_| "rendered PDF page could not be encoded as PNG")?;
    if image.len() > MAX_RENDERED_IMAGE_BYTES {
        return Err("rendered PDF page exceeds the 5 MiB image limit".into());
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)
        .map_err(|_| "renderer output could not be created")?;
    output
        .write_all(&image)
        .map_err(|_| "renderer output could not be written")?;
    output
        .flush()
        .map_err(|_| "renderer output could not be written")?;

    Ok(WorkerResponse::Rendered {
        page_count,
        page,
        image_width: u32::from(image_width),
        image_height: u32::from(image_height),
        image_bytes: image.len() as u64,
        image_sha256: format!("{:x}", Sha256::digest(&image)),
        annotations_omitted,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn bounded_render_dimensions(page_width: f32, page_height: f32) -> Result<(f32, u16, u16), String> {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
    {
        return Err("PDF page dimensions are invalid".into());
    }
    let page_width = f64::from(page_width);
    let page_height = f64::from(page_height);
    let edge_scale = f64::from(MAX_RENDER_LONG_EDGE) / page_width.max(page_height);
    let pixel_scale = (f64::from(MAX_RENDER_PIXELS) / (page_width * page_height)).sqrt();
    let scale = edge_scale.min(pixel_scale);
    if !scale.is_finite() || scale <= 0.0 {
        return Err("PDF page dimensions are invalid".into());
    }
    let image_width = (page_width * scale)
        .floor()
        .clamp(1.0, f64::from(MAX_RENDER_LONG_EDGE)) as u16;
    let image_height = (page_height * scale)
        .floor()
        .clamp(1.0, f64::from(MAX_RENDER_LONG_EDGE)) as u16;
    if u32::from(image_width) * u32::from(image_height) > MAX_RENDER_PIXELS {
        return Err("PDF page dimensions exceed the render pixel limit".into());
    }
    let scale = scale as f32;
    if !scale.is_finite() || scale <= 0.0 {
        return Err("PDF page dimensions are invalid".into());
    }
    Ok((scale, image_width, image_height))
}

fn load_validated_document(bytes: &[u8]) -> Result<Document, String> {
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
    Ok(document)
}

fn validated_page_count(document: &Document) -> Result<u32, String> {
    let page_count = document.page_iter().take(MAX_DOCUMENT_PAGES + 1).count();
    if page_count == 0 {
        return Err("PDF contains no pages".into());
    }
    if page_count > MAX_DOCUMENT_PAGES {
        return Err(format!("PDF contains more than {MAX_DOCUMENT_PAGES} pages"));
    }
    Ok(page_count as u32)
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
fn apply_worker_limits(max_file_bytes: u64) -> Result<(), String> {
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
        (Resource::Fsize, max_file_bytes),
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
fn apply_worker_limits(_: u64) -> Result<(), String> {
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

    const RGB_PIXELS: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];

    #[derive(Clone, Copy, Debug)]
    enum Nesting {
        Page,
        Form,
        TilingPattern,
        SoftMask,
        Type3Glyph,
    }

    fn image_pdf(image: Vec<u8>, filter: Option<&str>) -> Vec<u8> {
        nested_image_pdf(image, filter, Nesting::Page)
    }

    /// Draws one image on a page, directly or through a nested construct.
    fn nested_image_pdf(image: Vec<u8>, filter: Option<&str>, nesting: Nesting) -> Vec<u8> {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut image_dictionary = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 2,
            "Height" => 2,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
        };
        if let Some(filter) = filter {
            image_dictionary.set("Filter", filter);
        }
        let image_id = document.add_object(Stream::new(image_dictionary, image));
        let image_resources = dictionary! { "XObject" => dictionary! { "Im1" => image_id } };
        // Debug builds rasterise slowly, so nested content covers a small area.
        let draw_image = b"q 4 0 0 4 0 0 cm /Im1 Do Q".to_vec();
        let bounds = || -> Vec<Object> { vec![0.into(), 0.into(), 4.into(), 4.into()] };
        let (resources, content) = match nesting {
            Nesting::Page => (image_resources, draw_image),
            Nesting::Form => {
                let form_id = document.add_object(Stream::new(
                    dictionary! {
                        "Type" => "XObject",
                        "Subtype" => "Form",
                        "BBox" => bounds(),
                        "Resources" => image_resources,
                    },
                    draw_image,
                ));
                (
                    dictionary! { "XObject" => dictionary! { "Fm1" => form_id } },
                    b"/Fm1 Do".to_vec(),
                )
            }
            Nesting::TilingPattern => {
                let pattern_id = document.add_object(Stream::new(
                    dictionary! {
                        "Type" => "Pattern",
                        "PatternType" => 1,
                        "PaintType" => 1,
                        "TilingType" => 1,
                        "BBox" => bounds(),
                        "XStep" => 4,
                        "YStep" => 4,
                        "Resources" => image_resources,
                    },
                    draw_image,
                ));
                (
                    dictionary! { "Pattern" => dictionary! { "P1" => pattern_id } },
                    b"/Pattern cs /P1 scn 0 0 4 4 re f".to_vec(),
                )
            }
            Nesting::SoftMask => {
                let group_id = document.add_object(Stream::new(
                    dictionary! {
                        "Type" => "XObject",
                        "Subtype" => "Form",
                        "BBox" => bounds(),
                        "Group" => dictionary! { "S" => "Transparency", "CS" => "DeviceGray" },
                        "Resources" => image_resources,
                    },
                    draw_image,
                ));
                let state_id = document.add_object(dictionary! {
                    "Type" => "ExtGState",
                    "SMask" => dictionary! {
                        "Type" => "Mask",
                        "S" => "Luminosity",
                        "G" => group_id,
                    },
                });
                (
                    dictionary! { "ExtGState" => dictionary! { "GS1" => state_id } },
                    b"/GS1 gs 1 0 0 rg 0 0 4 4 re f".to_vec(),
                )
            }
            Nesting::Type3Glyph => {
                let glyph_id = document.add_object(Stream::new(
                    dictionary! {},
                    b"1 0 d0 q 1 0 0 1 0 0 cm /Im1 Do Q".to_vec(),
                ));
                let font_id = document.add_object(dictionary! {
                    "Type" => "Font",
                    "Subtype" => "Type3",
                    "FontBBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
                    "FontMatrix" => vec![
                        1.into(),
                        0.into(),
                        0.into(),
                        1.into(),
                        0.into(),
                        0.into(),
                    ],
                    "CharProcs" => dictionary! { "a" => glyph_id },
                    "Encoding" => dictionary! {
                        "Type" => "Encoding",
                        "Differences" => vec![97.into(), Object::Name(b"a".to_vec())],
                    },
                    "FirstChar" => 97,
                    "LastChar" => 97,
                    "Widths" => vec![1.into()],
                    "Resources" => image_resources,
                });
                (
                    dictionary! { "Font" => dictionary! { "F1" => font_id } },
                    b"BT /F1 4 Tf 0 0 Td (a) Tj ET".to_vec(),
                )
            }
        };
        let resources_id = document.add_object(resources);
        let content_id = document.add_object(Stream::new(dictionary! {}, content));
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
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
    fn view_arguments_are_normalized_and_bounded() {
        let normalized = normalized_view_arguments(&serde_json::json!({
            "url":"https://EXAMPLE.com:443/paper.pdf#page=2",
            "page":2,
            "expected_sha256":"AA00000000000000000000000000000000000000000000000000000000000000"
        }))
        .unwrap();
        assert_eq!(normalized["url"], "https://example.com/paper.pdf");
        assert_eq!(normalized["page"], 2);
        assert_eq!(
            normalized["expected_sha256"],
            "aa00000000000000000000000000000000000000000000000000000000000000"
        );
        for page in [0, MAX_DOCUMENT_PAGES as u32 + 1] {
            assert!(
                parse_view_arguments(&serde_json::json!({
                    "url":"https://example.com/paper.pdf",
                    "page":page
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn isolated_workers_share_the_sixteen_mib_pdf_limit() {
        let directory = tempfile::tempdir().unwrap();
        let accepted = vec![b'x'; 8 * 1024 * 1024 + 1];
        let accepted_path = directory.path().join("accepted.pdf");
        std::fs::write(&accepted_path, &accepted).unwrap();
        let digest = format!("{:x}", Sha256::digest(&accepted));
        assert_eq!(
            read_worker_input(&accepted_path, &digest).unwrap().len(),
            accepted.len()
        );

        let oversized_path = directory.path().join("oversized.pdf");
        let oversized = File::create(&oversized_path).unwrap();
        oversized
            .set_len(web::MAX_PDF_BODY_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            read_worker_input(&oversized_path, &digest).unwrap_err(),
            "PDF input exceeds the 16 MiB limit"
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
    fn rendering_is_single_page_bounded_and_self_describing() {
        let bytes = text_pdf(&["first", "second"]);
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.png");
        let WorkerResponse::Rendered {
            page_count,
            page,
            image_width,
            image_height,
            image_bytes,
            image_sha256,
            annotations_omitted,
            sha256,
        } = render_page(&bytes, 1, &first_path).unwrap()
        else {
            panic!("expected rendered page")
        };
        assert_eq!(annotations_omitted, 0);
        let first = std::fs::read(&first_path).unwrap();
        assert_eq!(page_count, 2);
        assert_eq!(page, 1);
        assert_eq!((image_width, image_height), (927, 1_200));
        assert_eq!(image_bytes, first.len() as u64);
        assert!(first.len() <= MAX_RENDERED_IMAGE_BYTES);
        assert_eq!(image_sha256, format!("{:x}", Sha256::digest(&first)));
        assert_eq!(sha256, format!("{:x}", Sha256::digest(&bytes)));
        validate_png(&first, image_width, image_height).unwrap();

        let second_path = directory.path().join("second.png");
        let WorkerResponse::Rendered {
            image_sha256: second_sha256,
            ..
        } = render_page(&bytes, 2, &second_path).unwrap()
        else {
            panic!("expected rendered page")
        };
        assert_ne!(image_sha256, second_sha256);
        assert!(std::fs::metadata(second_path).unwrap().len() > 0);
    }

    #[test]
    fn rendering_rejects_out_of_range_pages_without_creating_output() {
        let bytes = text_pdf(&["only"]);
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("page.png");
        assert!(
            render_page(&bytes, 2, &output)
                .unwrap_err()
                .contains("has 1 pages")
        );
        assert!(!output.exists());
    }

    #[test]
    fn rendering_fails_closed_when_an_embedded_image_cannot_decode() {
        let directory = tempfile::tempdir().unwrap();
        // Hayro 0.5 reports the first failure but omits the others silently.
        for (name, image, filter) in [
            ("dct", b"not-a-jpeg".to_vec(), Some("DCTDecode")),
            ("jbig2", b"not-jbig2".to_vec(), Some("JBIG2Decode")),
            ("empty", Vec::new(), None),
        ] {
            let output = directory.path().join(format!("{name}.png"));
            let error = render_page(&image_pdf(image, filter), 1, &output).unwrap_err();
            assert!(error.contains("faithfully"), "{name}: {error}");
            assert!(!output.exists(), "{name}");
        }

        let valid_output = directory.path().join("valid.png");
        render_page(&image_pdf(RGB_PIXELS.to_vec(), None), 1, &valid_output).unwrap();
        assert!(valid_output.exists());
    }

    #[test]
    fn rendering_audits_images_inside_nested_page_content() {
        let directory = tempfile::tempdir().unwrap();
        for nesting in [
            Nesting::Form,
            Nesting::TilingPattern,
            Nesting::SoftMask,
            Nesting::Type3Glyph,
        ] {
            let invalid_output = directory.path().join(format!("{nesting:?}-invalid.png"));
            let invalid = nested_image_pdf(b"not-jbig2".to_vec(), Some("JBIG2Decode"), nesting);
            let error = render_page(&invalid, 1, &invalid_output).unwrap_err();
            assert!(error.contains("faithfully"), "{nesting:?}: {error}");
            assert!(!invalid_output.exists(), "{nesting:?}");

            let valid_output = directory.path().join(format!("{nesting:?}-valid.png"));
            let valid = nested_image_pdf(RGB_PIXELS.to_vec(), None, nesting);
            render_page(&valid, 1, &valid_output).unwrap();
            assert!(valid_output.exists(), "{nesting:?}");
        }
    }

    /// Adds one visible annotation with the given appearance to the first page.
    fn annotated_pdf(base: &[u8], appearance: impl FnOnce(&mut Document) -> Object) -> Vec<u8> {
        let mut document = Document::load_mem(base).expect("load PDF");
        let appearance = appearance(&mut document);
        let annotation_id = document.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => vec![10.into(), 10.into(), 60.into(), 60.into()],
            "F" => 4,
            "AP" => dictionary! { "N" => appearance },
        });
        let page_id = *document.get_pages().get(&1).expect("first page");
        document
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .expect("page dictionary")
            .set("Annots", vec![Object::Reference(annotation_id)]);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("save PDF");
        bytes
    }

    fn filled_square(document: &mut Document) -> Object {
        Object::Reference(document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 50.into(), 50.into()],
            },
            b"0 0 1 rg 0 0 50 50 re f".to_vec(),
        )))
    }

    /// A checkbox selects one of several appearances, which Hayro 0.5 cannot draw.
    fn checkbox_states(document: &mut Document) -> Object {
        let checked = filled_square(document);
        Object::Dictionary(dictionary! { "Off" => filled_square(document), "Yes" => checked })
    }

    fn rendered(bytes: &[u8], directory: &Path, name: &str) -> Result<(String, u32), String> {
        let output = directory.join(format!("{name}.png"));
        match render_page(bytes, 1, &output)? {
            WorkerResponse::Rendered {
                image_sha256,
                annotations_omitted,
                ..
            } => Ok((image_sha256, annotations_omitted)),
            _ => panic!("expected rendered page"),
        }
    }

    #[test]
    fn rendering_leaves_out_and_counts_annotations_it_cannot_draw() {
        let directory = tempfile::tempdir().unwrap();
        let page = text_pdf(&["form"]);
        let (content_only, omitted) = rendered(&page, directory.path(), "content").unwrap();
        assert_eq!(omitted, 0);

        let drawn = annotated_pdf(&page, filled_square);
        let (with_annotation, omitted) = rendered(&drawn, directory.path(), "drawn").unwrap();
        assert_eq!(omitted, 0);
        assert_ne!(with_annotation, content_only);

        let form = annotated_pdf(&page, checkbox_states);
        let (without_annotation, omitted) = rendered(&form, directory.path(), "form").unwrap();
        assert_eq!(omitted, 1);
        assert_eq!(without_annotation, content_only);
    }

    #[test]
    fn rendering_never_excuses_missing_page_content_as_an_annotation() {
        let directory = tempfile::tempdir().unwrap();
        let broken_content = image_pdf(b"not-jbig2".to_vec(), Some("JBIG2Decode"));
        for (name, bytes) in [
            ("drawn", annotated_pdf(&broken_content, filled_square)),
            ("form", annotated_pdf(&broken_content, checkbox_states)),
        ] {
            let error = rendered(&bytes, directory.path(), name).unwrap_err();
            assert!(error.contains("faithfully"), "{name}: {error}");
            assert!(!directory.path().join(format!("{name}.png")).exists());
        }

        let broken_annotation = annotated_pdf(&text_pdf(&["form"]), |document| {
            Object::Reference(document.add_object(Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 50.into(), 50.into()],
                    "Filter" => "FlateDecode",
                },
                b"not-deflate".to_vec(),
            )))
        });
        let (_, omitted) = rendered(&broken_annotation, directory.path(), "annotation").unwrap();
        assert_eq!(omitted, 1);
    }

    #[test]
    fn render_dimensions_bound_both_edge_and_total_pixels() {
        let (_, width, height) = bounded_render_dimensions(1_200.0, 1_200.0).unwrap();
        assert_eq!((width, height), (1_095, 1_095));
        assert!(u32::from(width) * u32::from(height) <= MAX_RENDER_PIXELS);
        assert!(width <= MAX_RENDER_LONG_EDGE && height <= MAX_RENDER_LONG_EDGE);

        let (_, width, height) = bounded_render_dimensions(612.0, 792.0).unwrap();
        assert_eq!((width, height), (927, 1_200));
        assert!(u32::from(width) * u32::from(height) <= MAX_RENDER_PIXELS);
    }

    #[test]
    fn sanitation_removes_terminal_controls_without_losing_lines() {
        assert_eq!(sanitize_text("a\r\nb\t\u{1b}[31m"), "a\nb [31m");
    }
}
