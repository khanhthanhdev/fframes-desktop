//! Bounded, resumable downloads for artifacts authorized by a verified release manifest.
//!
//! `curl` is invoked as a fixed platform executable in an owned process scope. The
//! downloaded bytes are streamed to an app-owned `.part` file, bounded by the signed
//! size, hashed before publication, and atomically linked into the content-addressed
//! cache. Redirects must remain HTTPS and on the signed URL's origin.

use crate::{
    ReleaseArtifact, VerifiedReleaseManifest,
    disk::{DiskSpaceError, ensure_available},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use studio_bootstrap::{ProcessTreeManager, SpawnOptions};
use thiserror::Error;

const MAX_REDIRECTS: &str = "5";
const CONNECT_TIMEOUT_SECONDS: &str = "15";
const TRANSFER_TIMEOUT_SECONDS: &str = "3600";
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("artifact URL is not a valid HTTPS URL")]
    InvalidUrl,
    #[error("download cancelled")]
    Cancelled,
    #[error("download owner is shutting down")]
    OwnerShuttingDown,
    #[error("download failed; resumable partial retained: {0}")]
    Transport(String),
    #[error("redirect left the signed artifact origin")]
    RedirectOriginChanged,
    #[error("download exceeded its signed size limit")]
    SizeLimit,
    #[error("download size mismatch: expected {expected}, received {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("download checksum mismatch: expected {expected}, actual {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("cached artifact at {0} exists but does not match the signed identity")]
    InvalidCacheEntry(PathBuf),
    #[error(transparent)]
    DiskSpace(#[from] DiskSpaceError),
    #[error("I/O error while acquiring artifact: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseArtifactKind {
    App,
    Sdk,
}

pub struct SdkDownloader {
    cache_dir: PathBuf,
}

impl SdkDownloader {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
        }
    }

    /// Acquires the app or SDK artifact only from an authenticated release token.
    /// Progress reports actual cached bytes, including bytes already present on resume.
    pub fn download(
        &self,
        release: &VerifiedReleaseManifest,
        kind: ReleaseArtifactKind,
        processes: &ProcessTreeManager,
        cancelled: &impl Fn() -> bool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, DownloadError> {
        let artifact = match kind {
            ReleaseArtifactKind::App => release.app_artifact(),
            ReleaseArtifactKind::Sdk => release.sdk_artifact(),
        };
        self.download_spec(artifact, processes, cancelled, &mut progress, false)
    }

    fn download_spec(
        &self,
        artifact: &ReleaseArtifact,
        processes: &ProcessTreeManager,
        cancelled: &impl Fn() -> bool,
        progress: &mut impl FnMut(u64, u64),
        allow_loopback_http: bool,
    ) -> Result<PathBuf, DownloadError> {
        let origin =
            parse_origin(&artifact.url, allow_loopback_http).ok_or(DownloadError::InvalidUrl)?;
        validate_digest(&artifact.sha256).ok_or(DownloadError::InvalidUrl)?;
        if artifact.size_bytes == 0 {
            return Err(DownloadError::InvalidUrl);
        }
        fs::create_dir_all(&self.cache_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.cache_dir, fs::Permissions::from_mode(0o700))?;
        }
        let digest = artifact.sha256.to_ascii_lowercase();
        let destination = self.cache_dir.join(format!("{digest}.artifact"));
        if destination.exists() {
            verify_cached(&destination, artifact)?;
            progress(artifact.size_bytes, artifact.size_bytes);
            return Ok(destination);
        }
        let partial = self.cache_dir.join(format!("{digest}.part"));
        if fs::metadata(&partial)
            .map(|metadata| metadata.len() > artifact.size_bytes)
            .unwrap_or(false)
        {
            fs::remove_file(&partial)?;
        }
        let resumed_bytes = fs::metadata(&partial).map_or(0, |metadata| metadata.len());
        const CACHE_RESERVE_BYTES: u64 = 32 * 1024 * 1024;
        ensure_available(
            &self.cache_dir,
            artifact
                .size_bytes
                .saturating_sub(resumed_bytes)
                .saturating_add(CACHE_RESERVE_BYTES),
        )?;

        let scope = processes.sub_manager();
        let mut resumed = partial.exists();
        loop {
            if cancelled() || scope.is_shutdown() {
                return Err(DownloadError::Cancelled);
            }
            let metadata_file = tempfile::NamedTempFile::new_in(&self.cache_dir)?;
            let metadata_path = metadata_file.path().to_owned();
            let mut options = SpawnOptions::new(curl_program().ok_or_else(|| {
                DownloadError::Transport(
                    "the platform HTTPS transfer client (curl) is unavailable".into(),
                )
            })?);
            options.args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--proto",
                if allow_loopback_http {
                    "=http,https"
                } else {
                    "=https"
                },
                "--proto-redir",
                "=https",
                "--max-redirs",
                MAX_REDIRECTS,
                "--connect-timeout",
                CONNECT_TIMEOUT_SECONDS,
                "--max-time",
                TRANSFER_TIMEOUT_SECONDS,
                "--output",
            ]);
            options.arg(&partial);
            if resumed {
                options.args(["--continue-at", "-"]);
            }
            options.args(["--write-out", "%{url_effective}\n%{http_code}\n"]);
            options.arg("--");
            options.arg(&artifact.url);
            options.stdout(Stdio::from(metadata_file.as_file().try_clone()?));
            options.stderr(Stdio::null());
            let child = scope.spawn(options).map_err(|error| {
                DownloadError::Transport(format!("could not start HTTPS transfer: {error}"))
            })?;

            let started = Instant::now();
            let mut restart_without_range = false;
            let status = loop {
                if cancelled() || scope.is_shutdown() {
                    let _ = child.lock().terminate_verified(Duration::from_millis(300));
                    return Err(if scope.is_shutdown() {
                        DownloadError::OwnerShuttingDown
                    } else {
                        DownloadError::Cancelled
                    });
                }
                let child_status = child
                    .lock()
                    .try_wait()
                    .map_err(|error| DownloadError::Transport(error.to_string()))?;
                if let Some(status) = child_status {
                    break Some(status);
                }
                let actual = fs::metadata(&partial).map_or(0, |metadata| metadata.len());
                if actual > artifact.size_bytes {
                    let _ = child.lock().terminate_verified(Duration::from_millis(300));
                    let _ = fs::remove_file(&partial);
                    if resumed {
                        resumed = false;
                        restart_without_range = true;
                        break None;
                    }
                    return Err(DownloadError::SizeLimit);
                }
                progress(actual, artifact.size_bytes);
                if started.elapsed() > Duration::from_secs(3600) {
                    let _ = child.lock().terminate_verified(Duration::from_millis(300));
                    return Err(DownloadError::Transport("transfer timed out".into()));
                }
                thread::sleep(POLL_INTERVAL);
            };
            if restart_without_range {
                continue;
            }
            let status = status.expect("a completed transfer has an exit status");

            let metadata = fs::read_to_string(&metadata_path).unwrap_or_default();
            let effective_url = metadata.lines().next().unwrap_or_default();
            if parse_origin(effective_url, allow_loopback_http).as_ref() != Some(&origin) {
                let _ = fs::remove_file(&partial);
                return Err(DownloadError::RedirectOriginChanged);
            }
            if !status.success() {
                // libcurl error 33 is CURLE_RANGE_ERROR: the origin rejected or
                // ignored the resume request. Retry once from byte zero, rather than
                // repeatedly preserving a prefix the server cannot resume.
                if resumed && status.code() == Some(33) {
                    fs::remove_file(&partial)?;
                    resumed = false;
                    continue;
                }
                return Err(DownloadError::Transport(format!(
                    "transfer client exited with {}",
                    status
                        .code()
                        .map_or_else(|| "a signal".into(), |code| code.to_string())
                )));
            }

            let actual_size = fs::metadata(&partial).map_or(0, |metadata| metadata.len());
            if resumed && actual_size > artifact.size_bytes {
                // Some servers ignore Range and return the complete representation;
                // never accept concatenated bytes. Drop that partial and do one clean retry.
                fs::remove_file(&partial)?;
                resumed = false;
                continue;
            }
            if actual_size != artifact.size_bytes {
                return Err(DownloadError::SizeMismatch {
                    expected: artifact.size_bytes,
                    actual: actual_size,
                });
            }
            let actual_hash = hash_file(&partial)?;
            if !actual_hash.eq_ignore_ascii_case(&artifact.sha256) {
                fs::remove_file(&partial)?;
                return Err(DownloadError::ChecksumMismatch {
                    expected: artifact.sha256.clone(),
                    actual: actual_hash,
                });
            }
            File::open(&partial)?.sync_all()?;
            match fs::hard_link(&partial, &destination) {
                Ok(()) => {
                    fs::remove_file(&partial)?;
                    sync_directory(&self.cache_dir)?;
                    progress(artifact.size_bytes, artifact.size_bytes);
                    return Ok(destination);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    verify_cached(&destination, artifact)?;
                    fs::remove_file(&partial)?;
                    return Ok(destination);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn parse_origin(url: &str, allow_loopback_http: bool) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && !(allow_loopback_http && scheme == "http") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') || authority.contains('\\') {
        return None;
    }
    let authority = authority.to_ascii_lowercase();
    if scheme == "http"
        && !authority.starts_with("127.0.0.1:")
        && !authority.starts_with("localhost:")
        && !authority.starts_with("[::1]:")
    {
        return None;
    }
    Some((scheme, authority))
}

fn validate_digest(value: &str) -> Option<()> {
    (value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(())
}

fn verify_cached(path: &Path, artifact: &ReleaseArtifact) -> Result<(), DownloadError> {
    let size = fs::metadata(path)?.len();
    let digest = hash_file(path)?;
    if size != artifact.size_bytes || !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(DownloadError::InvalidCacheEntry(path.to_owned()));
    }
    Ok(())
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(target_os = "linux")]
fn curl_program() -> Option<PathBuf> {
    ["/usr/bin/curl", "/bin/curl"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

#[cfg(target_os = "macos")]
fn curl_program() -> Option<PathBuf> {
    let path = PathBuf::from("/usr/bin/curl");
    path.is_file().then_some(path)
}

#[cfg(target_os = "windows")]
fn curl_program() -> Option<PathBuf> {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32/curl.exe"))
        .filter(|path| path.is_file())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn curl_program() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct TestServer {
        requests: Arc<AtomicUsize>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn new(body: Vec<u8>, honor_range: bool) -> (Self, String) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let count = requests.clone();
            let thread = thread::spawn(move || {
                let mut last_request = Instant::now();
                loop {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if last_request.elapsed() > Duration::from_secs(2) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => break,
                    };
                    last_request = Instant::now();
                    count.fetch_add(1, Ordering::SeqCst);
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut request = [0; 8192];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let request = String::from_utf8_lossy(&request[..read]);
                    let range = request.lines().find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .and_then(|value| value.trim().split('-').next())
                            .and_then(|value| value.parse::<usize>().ok())
                    });
                    let offset = if honor_range {
                        range.unwrap_or(0).min(body.len())
                    } else {
                        0
                    };
                    let status = if offset > 0 {
                        "206 Partial Content"
                    } else {
                        "200 OK"
                    };
                    let headers = if offset > 0 {
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body.len() - offset,
                            offset,
                            body.len() - 1,
                            body.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                    };
                    if stream.write_all(headers.as_bytes()).is_ok() {
                        let _ = stream.write_all(&body[offset..]);
                    }
                }
            });
            (
                Self {
                    requests,
                    thread: Some(thread),
                },
                format!("http://{address}/artifact"),
            )
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn artifact(url: String, body: &[u8]) -> ReleaseArtifact {
        ReleaseArtifact {
            url,
            sha256: format!("{:x}", Sha256::digest(body)),
            size_bytes: body.len() as u64,
        }
    }

    #[test]
    fn resumes_partial_and_publishes_only_hash_verified_bytes() {
        if curl_program().is_none() {
            return;
        }
        let bytes =
            b"a verified test artifact with enough bytes to exercise a range request".to_vec();
        let (server, url) = TestServer::new(bytes.clone(), true);
        let cache = tempfile::tempdir().unwrap();
        let artifact = artifact(url, &bytes);
        let partial = cache.path().join(format!("{}.part", artifact.sha256));
        fs::write(&partial, &bytes[..bytes.len() / 2]).unwrap();
        let downloader = SdkDownloader::new(cache.path());
        let process_tree = ProcessTreeManager::new();
        let result = downloader
            .download_spec(&artifact, &process_tree, &|| false, &mut |_, _| {}, true)
            .unwrap();
        assert_eq!(fs::read(&result).unwrap(), bytes);
        assert!(!partial.exists());
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        process_tree.terminate_all(Duration::from_millis(10));
    }

    #[test]
    fn ignored_range_is_discarded_and_restarted_cleanly() {
        if curl_program().is_none() {
            return;
        }
        let bytes = b"the server ignores Range, so a clean retry is required".to_vec();
        let (server, url) = TestServer::new(bytes.clone(), false);
        let cache = tempfile::tempdir().unwrap();
        let artifact = artifact(url, &bytes);
        let partial = cache.path().join(format!("{}.part", artifact.sha256));
        fs::write(&partial, b"stale prefix").unwrap();
        let downloader = SdkDownloader::new(cache.path());
        let process_tree = ProcessTreeManager::new();
        let result = downloader
            .download_spec(&artifact, &process_tree, &|| false, &mut |_, _| {}, true)
            .unwrap();
        assert_eq!(fs::read(&result).unwrap(), bytes);
        assert!((1..=2).contains(&server.requests.load(Ordering::SeqCst)));
        process_tree.terminate_all(Duration::from_millis(10));
    }

    #[test]
    fn artifact_urls_reject_credentials_and_non_https_origins() {
        assert!(parse_origin("https://user@example.test/file", false).is_none());
        assert!(parse_origin("http://example.test/file", false).is_none());
        assert!(parse_origin("http://127.0.0.1:8080/file", true).is_some());
    }
}
