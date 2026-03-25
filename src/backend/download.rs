use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::Sender,
    time::Duration,
};

use anyhow::{anyhow, bail, Context};
use chrono::Local;
use percent_encoding::percent_decode_str;
use reqwest::{
    header::{self, HeaderMap},
    redirect::Policy,
    Client, Response, Url,
};
use tokio_util::sync::CancellationToken;

const VALID_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "ico", "svg", "ppm", "pgm", "pbm",
    "pam", "hdr", "exr", "ff", "avif", "jxl",
];

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub destination_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub enum DownloadProgressEvent {
    Started {
        final_path: PathBuf,
        total_bytes: Option<u64>,
    },
    Advanced {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Finished {
        saved_path: PathBuf,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

#[derive(Debug)]
pub enum DownloadError {
    Cancelled,
    Failed(anyhow::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Download cancelled"),
            Self::Failed(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DownloadError {}

pub async fn download_image_async(
    request: DownloadRequest,
    progress_tx: &Sender<DownloadProgressEvent>,
    cancel: CancellationToken,
) -> Result<PathBuf, DownloadError> {
    let result = download_image_inner_async(&request, progress_tx, &cancel).await;
    match &result {
        Ok(saved_path) => {
            let _ = progress_tx.send(DownloadProgressEvent::Finished {
                saved_path: saved_path.clone(),
            });
        }
        Err(DownloadError::Cancelled) => {
            let _ = progress_tx.send(DownloadProgressEvent::Cancelled);
        }
        Err(DownloadError::Failed(error)) => {
            let _ = progress_tx.send(DownloadProgressEvent::Failed {
                message: error.to_string(),
            });
        }
    }
    result
}

async fn download_image_inner_async(
    request: &DownloadRequest,
    progress_tx: &Sender<DownloadProgressEvent>,
    cancel: &CancellationToken,
) -> Result<PathBuf, DownloadError> {
    let url = validate_download_url(&request.url).map_err(DownloadError::Failed)?;
    if !request.destination_dir.is_dir() {
        return Err(DownloadError::Failed(anyhow!(
            "Destination is not a directory: {}",
            request.destination_dir.display()
        )));
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }

    let client = build_client().map_err(DownloadError::Failed)?;
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
        response = client.get(url.clone()).send() => response,
    }
    .and_then(Response::error_for_status)
    .map_err(|error: reqwest::Error| DownloadError::Failed(error.into()))?;

    validate_content_type(response.headers()).map_err(DownloadError::Failed)?;
    let final_path = resolve_download_path(&url, response.headers(), &request.destination_dir)
        .map_err(DownloadError::Failed)?;
    let total_bytes = response.content_length();
    let _ = progress_tx.send(DownloadProgressEvent::Started {
        final_path: final_path.clone(),
        total_bytes,
    });

    let partial_path = partial_download_path(&final_path);
    let mut partial_file = File::create(&partial_path)
        .with_context(|| {
            format!(
                "Could not create partial download file at {}",
                partial_path.display()
            )
        })
        .map_err(DownloadError::Failed)?;
    if cancel.is_cancelled() {
        cleanup_partial(&partial_path);
        return Err(DownloadError::Cancelled);
    }

    let mut response = response;
    let stream_result = stream_response_to_file_async(
        &mut response,
        &mut partial_file,
        total_bytes,
        progress_tx,
        cancel,
    )
    .await;
    match stream_result {
        Ok(()) => {
            if cancel.is_cancelled() {
                cleanup_partial(&partial_path);
                return Err(DownloadError::Cancelled);
            }
            partial_file
                .flush()
                .and_then(|_| partial_file.sync_all())
                .with_context(|| {
                    format!(
                        "Could not finalize partial download file at {}",
                        partial_path.display()
                    )
                })
                .map_err(DownloadError::Failed)?;
            if cancel.is_cancelled() {
                cleanup_partial(&partial_path);
                return Err(DownloadError::Cancelled);
            }
            fs::rename(&partial_path, &final_path)
                .with_context(|| {
                    format!(
                        "Could not move downloaded image into place: {}",
                        final_path.display()
                    )
                })
                .map_err(DownloadError::Failed)?;
            Ok(final_path)
        }
        Err(error) => {
            cleanup_partial(&partial_path);
            Err(error)
        }
    }
}

fn build_client() -> anyhow::Result<Client> {
    Client::builder()
        .redirect(Policy::limited(10))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .user_agent(format!("walt/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Could not initialize HTTP client")
}

pub fn validate_download_url(raw: &str) -> anyhow::Result<Url> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Enter an image URL");
    }

    let url = Url::parse(trimmed).context("Enter a valid HTTP or HTTPS URL")?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        _ => bail!("Only HTTP and HTTPS image URLs are supported"),
    }
}

fn validate_content_type(headers: &HeaderMap) -> anyhow::Result<()> {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };

    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("image/") {
        Ok(())
    } else {
        bail!("URL did not return an image");
    }
}

fn resolve_download_path(
    url: &Url,
    headers: &HeaderMap,
    destination_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(normalize_content_type);
    let header_filename = content_disposition_filename(headers);
    let fallback_extension = content_type
        .as_deref()
        .and_then(extension_from_content_type);

    let resolved_name = [header_filename, url_filename(url)]
        .into_iter()
        .flatten()
        .find_map(|candidate| sanitize_candidate_filename(&candidate, fallback_extension).ok())
        .or_else(|| fallback_extension.map(fallback_download_filename))
        .ok_or_else(|| {
            anyhow!("Could not determine a supported filename for the downloaded image")
        })?;

    Ok(unique_download_path(destination_dir, &resolved_name))
}

fn content_disposition_filename(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::CONTENT_DISPOSITION)?.to_str().ok()?;
    let mut filename_star = None;
    let mut filename = None;

    for segment in value.split(';') {
        let segment = segment.trim();
        if let Some(rest) = segment.strip_prefix("filename*=") {
            filename_star = decode_rfc5987_filename(rest);
            continue;
        }
        if let Some(rest) = segment.strip_prefix("filename=") {
            filename = Some(trim_quotes(rest));
        }
    }

    filename_star.or(filename)
}

fn decode_rfc5987_filename(value: &str) -> Option<String> {
    let trimmed = trim_quotes(value);
    let mut parts = trimmed.splitn(3, '\'');
    let charset = parts.next().unwrap_or_default();
    let _language = parts.next();
    let encoded = parts.next().unwrap_or(trimmed.as_str());
    let decoded_bytes = percent_decode_str(encoded).collect::<Vec<_>>();

    if charset.is_empty() || charset.eq_ignore_ascii_case("utf-8") {
        return String::from_utf8(decoded_bytes).ok().or(Some(trimmed));
    }

    Some(trimmed)
}

fn trim_quotes(value: &str) -> String {
    value.trim_matches('"').to_string()
}

fn url_filename(url: &Url) -> Option<String> {
    let candidate = url
        .path_segments()
        .and_then(|segments| segments.last())
        .filter(|segment| !segment.trim().is_empty())?;
    Some(candidate.to_string())
}

fn sanitize_candidate_filename(
    candidate: &str,
    fallback_extension: Option<&str>,
) -> anyhow::Result<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        bail!("empty candidate filename");
    }

    let last_segment = candidate
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(candidate)
        .trim();
    if last_segment.is_empty() {
        bail!("empty candidate filename");
    }

    let path = Path::new(last_segment);
    let raw_stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .trim();
    let stem = sanitize_filename_stem(raw_stem);
    if stem.is_empty() {
        bail!("empty candidate filename");
    }

    let raw_extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let extension = if is_supported_extension(raw_extension) {
        raw_extension.to_ascii_lowercase()
    } else if let Some(fallback_extension) = fallback_extension {
        fallback_extension.to_string()
    } else {
        bail!("unsupported filename extension");
    };

    Ok(format!("{stem}.{extension}"))
}

fn sanitize_filename_stem(stem: &str) -> String {
    let mut sanitized = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') {
            sanitized.push(ch);
        } else if !sanitized.ends_with('_') {
            sanitized.push('_');
        }
    }

    sanitized
        .trim_matches(|ch: char| ch == '.' || ch == ' ' || ch == '_')
        .to_string()
}

fn normalize_content_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase()
}

fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        "image/tiff" => Some("tiff"),
        "image/webp" => Some("webp"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        "image/svg+xml" => Some("svg"),
        "image/avif" => Some("avif"),
        "image/jxl" => Some("jxl"),
        "image/vnd.radiance" => Some("hdr"),
        "image/x-exr" => Some("exr"),
        "image/x-portable-pixmap" => Some("ppm"),
        "image/x-portable-graymap" => Some("pgm"),
        "image/x-portable-bitmap" => Some("pbm"),
        "image/x-portable-arbitrarymap" => Some("pam"),
        _ => None,
    }
}

fn fallback_download_filename(extension: &str) -> String {
    format!(
        "download-{}.{}",
        Local::now().format("%Y%m%d-%H%M%S"),
        extension
    )
}

fn is_supported_extension(extension: &str) -> bool {
    VALID_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
}

fn unique_download_path(destination_dir: &Path, file_name: &str) -> PathBuf {
    let initial = destination_dir.join(file_name);
    if !initial.exists() {
        return initial;
    }

    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("download");
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();

    for index in 1.. {
        let candidate = if extension.is_empty() {
            destination_dir.join(format!("{stem}-{index}"))
        } else {
            destination_dir.join(format!("{stem}-{index}.{extension}"))
        };
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("download path counter is unbounded");
}

fn partial_download_path(final_path: &Path) -> PathBuf {
    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    final_path.with_file_name(format!(".{file_name}.part-{}", std::process::id()))
}

async fn stream_response_to_file_async(
    response: &mut Response,
    partial_file: &mut File,
    total_bytes: Option<u64>,
    progress_tx: &Sender<DownloadProgressEvent>,
    cancel: &CancellationToken,
) -> Result<(), DownloadError> {
    let mut downloaded_bytes = 0_u64;

    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            chunk = response.chunk() => chunk,
        }
        .map_err(|error: reqwest::Error| DownloadError::Failed(error.into()))?;

        let Some(chunk) = chunk else {
            break;
        };

        partial_file
            .write_all(&chunk)
            .map_err(|error| DownloadError::Failed(error.into()))?;
        downloaded_bytes += chunk.len() as u64;
        let _ = progress_tx.send(DownloadProgressEvent::Advanced {
            downloaded_bytes,
            total_bytes,
        });
    }

    Ok(())
}

fn cleanup_partial(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::{
        content_disposition_filename, download_image_async, extension_from_content_type,
        partial_download_path, resolve_download_path, sanitize_candidate_filename,
        unique_download_path, validate_content_type, validate_download_url, DownloadError,
        DownloadProgressEvent, DownloadRequest,
    };
    use reqwest::header::{HeaderMap, HeaderValue, CONTENT_DISPOSITION, CONTENT_TYPE};
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        sync::mpsc,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tokio::runtime::Builder;
    use tokio_util::sync::CancellationToken;

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("walt-download-test-{unique}"));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
    }

    #[test]
    fn rejects_non_http_urls() {
        let error = validate_download_url("file:///tmp/test.png")
            .expect_err("non-http url should fail")
            .to_string();
        assert!(error.contains("Only HTTP and HTTPS"));
    }

    #[test]
    fn pulls_filename_from_content_disposition() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"wallpaper.png\""),
        );

        assert_eq!(
            content_disposition_filename(&headers),
            Some("wallpaper.png".to_string())
        );
    }

    #[test]
    fn content_disposition_filename_prefers_and_decodes_filename_star() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static(
                "attachment; filename=\"fallback.jpg\"; filename*=UTF-8''hello%20world.jpg",
            ),
        );

        assert_eq!(
            content_disposition_filename(&headers),
            Some("hello world.jpg".to_string())
        );
    }

    #[test]
    fn content_disposition_filename_handles_invalid_percent_encoding_without_panicking() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename*=UTF-8''bad%ZZname.jpg"),
        );

        assert_eq!(
            content_disposition_filename(&headers),
            Some("bad%ZZname.jpg".to_string())
        );
    }

    #[test]
    fn sanitizes_and_normalizes_candidate_filenames() {
        assert_eq!(
            sanitize_candidate_filename("cats/photo.JPG", Some("png")).expect("sanitize"),
            "photo.jpg"
        );
        assert_eq!(
            sanitize_candidate_filename("weird<>name", Some("png")).expect("sanitize"),
            "weird_name.png"
        );
    }

    #[test]
    fn maps_known_content_types_to_extensions() {
        assert_eq!(extension_from_content_type("image/jpeg"), Some("jpg"));
        assert_eq!(extension_from_content_type("image/svg+xml"), Some("svg"));
        assert_eq!(extension_from_content_type("text/html"), None);
    }

    #[test]
    fn rejects_non_image_content_type() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );

        let error = validate_content_type(&headers)
            .expect_err("text/html should fail")
            .to_string();
        assert!(error.contains("did not return an image"));
    }

    #[test]
    fn resolves_collision_by_auto_renaming() {
        let root = temp_dir();
        let existing = root.join("wallpaper.png");
        fs::write(&existing, b"present").expect("write existing file");

        let unique = unique_download_path(&root, "wallpaper.png");
        assert_eq!(unique, root.join("wallpaper-1.png"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn resolves_final_path_from_headers_and_url() {
        let root = temp_dir();
        let url = reqwest::Url::parse("https://example.com/download").expect("parse url");
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"lake-view.webp\""),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/webp"));

        let resolved = resolve_download_path(&url, &headers, &root).expect("resolve path");
        assert_eq!(resolved, root.join("lake-view.webp"));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn partial_download_file_uses_hidden_sidecar_name() {
        let partial = partial_download_path(PathBuf::from("/tmp/wallpaper.png").as_path());
        assert!(partial
            .file_name()
            .and_then(|name| name.to_str())
            .expect("filename")
            .starts_with(".wallpaper.png.part-"));
    }

    #[test]
    fn downloads_cleanup_partial_files_when_cancelled() {
        let root = temp_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("local addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request_buffer = [0_u8; 1024];
            let _ = stream.read(&mut request_buffer);
            if stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 999999\r\n\r\n",
                )
                .is_err()
            {
                return;
            }
            for _ in 0..128 {
                if stream.write_all(&[0_u8; 1024]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });

        let request = DownloadRequest {
            url: format!("http://{address}/wallpaper.png"),
            destination_dir: root.clone(),
        };
        let (progress_tx, progress_rx) = mpsc::channel::<DownloadProgressEvent>();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let worker = thread::spawn(move || {
            test_runtime().block_on(download_image_async(request, &progress_tx, cancel))
        });

        while let Ok(event) = progress_rx.recv() {
            if matches!(event, DownloadProgressEvent::Advanced { .. }) {
                cancel_clone.cancel();
                break;
            }
        }

        let result = worker.join().expect("join worker");
        assert!(matches!(result, Err(DownloadError::Cancelled)));
        let entries = fs::read_dir(&root)
            .expect("read dir")
            .map(|entry| entry.expect("entry").path())
            .collect::<Vec<_>>();
        assert!(entries.is_empty(), "partial files should be cleaned up");

        server.join().expect("join server");
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn download_cancelled_before_headers_returns_cancelled() {
        let root = temp_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("local addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request_buffer = [0_u8; 1024];
            let _ = stream.read(&mut request_buffer);
            thread::sleep(Duration::from_secs(2));
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\n\r\nbody",
            );
        });

        let request = DownloadRequest {
            url: format!("http://{address}/wallpaper.png"),
            destination_dir: root.clone(),
        };
        let (progress_tx, _progress_rx) = mpsc::channel::<DownloadProgressEvent>();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel_clone.cancel();
        });

        let started = std::time::Instant::now();
        let result = test_runtime().block_on(download_image_async(request, &progress_tx, cancel));

        assert!(matches!(result, Err(DownloadError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(fs::read_dir(&root).expect("read dir").next().is_none());

        worker.join().expect("join cancel thread");
        server.join().expect("join server");
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn download_success_emits_started_advanced_finished_once() {
        let root = temp_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("local addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request_buffer = [0_u8; 1024];
            let _ = stream.read(&mut request_buffer);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nContent-Disposition: attachment; filename=\"ok.png\"\r\n\r\nbody",
            );
        });

        let request = DownloadRequest {
            url: format!("http://{address}/wallpaper.png"),
            destination_dir: root.clone(),
        };
        let (progress_tx, progress_rx) = mpsc::channel::<DownloadProgressEvent>();
        let cancel = CancellationToken::new();

        let result = test_runtime().block_on(download_image_async(request, &progress_tx, cancel));
        assert!(result.is_ok());

        let events = progress_rx.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            events.first(),
            Some(DownloadProgressEvent::Started { .. })
        ));
        assert!(events
            .iter()
            .any(|event| matches!(event, DownloadProgressEvent::Advanced { .. })));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, DownloadProgressEvent::Finished { .. }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, DownloadProgressEvent::Cancelled))
                .count(),
            0
        );

        server.join().expect("join server");
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
