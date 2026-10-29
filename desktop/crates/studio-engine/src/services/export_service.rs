use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::FrozenExportSource;
use crate::build_materialization::MaterializedBuild;
use fframes_media::{AudioDecoder, FFmpegDecoder, probe_streams};
use fframes_studio_protocol::MAX_TIMING_DOC_BYTES;
use fframes_studio_protocol::export::{
    EXPORT_CAPABILITIES_FLAG, EXPORT_CAPABILITIES_VERSION, EXPORT_FLAG, ExportCapabilities,
    ExportContainer, ExportSettings,
};
use studio_bootstrap::{ProcessTreeManager, SpawnOptions};

use super::{
    build_service::{BuildService, Subscriber, SubscriberKind},
    worker_project::{compile_portable_worker_via, worker_binary},
};

const PROGRESS_LINE_LIMIT: usize = 64 * 1024;
const DIAGNOSTIC_LIMIT: usize = 32;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const CAPABILITIES_LIMIT: u64 = 64 * 1024;
const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(30);
const VERIFY_FRAME_RATE: usize = 30;
const EXPORT_ACTIVE: u8 = 0;
const EXPORT_CANCELLED: u8 = 1;
const EXPORT_PUBLISHING: u8 = 2;
const EXPORT_PUBLISHED: u8 = 3;

/// Cancellation and no-clobber publication share one atomic commit fence.
#[derive(Default)]
pub struct ExportControl {
    state: AtomicU8,
}

impl ExportControl {
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == EXPORT_CANCELLED
    }

    /// Returns true only when cancellation won before the publication fence.
    pub fn request_cancel(&self) -> bool {
        self.state
            .compare_exchange(
                EXPORT_ACTIVE,
                EXPORT_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub fn is_publishing(&self) -> bool {
        self.state.load(Ordering::Acquire) == EXPORT_PUBLISHING
    }

    pub(crate) fn begin_publication(&self) -> bool {
        self.state
            .compare_exchange(
                EXPORT_ACTIVE,
                EXPORT_PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(crate) fn finish_publication(&self) {
        self.state.store(EXPORT_PUBLISHED, Ordering::Release);
    }

    /// A publication that failed: the export is active again until it reports the
    /// failure, so a Cancel in between still counts.
    pub(crate) fn abandon_publication(&self) {
        self.state.store(EXPORT_ACTIVE, Ordering::Release);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportProgress {
    Started {
        revision: String,
        label: String,
    },
    Rendering {
        done: usize,
        total: usize,
    },
    Audio,
    Warning(String),
    Verifying,
    /// A slide deck export decodes its stop frames into posters and thumbnails.
    Posters {
        done: usize,
        total: usize,
    },
    /// A slide deck export writes its page and manifest and publishes the folder.
    WritingDeck,
    Complete {
        destination: PathBuf,
        revision: String,
    },
    Failed(String),
    Cancelled,
}

pub struct ExportRequest {
    pub source: FrozenExportSource,
    pub sdk: PathBuf,
    pub builds: PathBuf,
    pub destination: PathBuf,
    /// Container, codec, size and frame rate. Anything but the default needs a worker
    /// that reports export capabilities; older workers only export the default.
    pub settings: ExportSettings,
    /// The timing document to export with; `None` exports the scenes in code order.
    pub timing_json: Option<String>,
    /// The props document to export with; `None` exports the code's values.
    pub props_json: Option<String>,
}

/// What [`render_verified`] renders: [`ExportRequest`] without its destination.
pub struct RenderRequest {
    pub source: FrozenExportSource,
    pub sdk: PathBuf,
    pub builds: PathBuf,
    pub settings: ExportSettings,
    pub timing_json: Option<String>,
    pub props_json: Option<String>,
}

/// Where [`render_verified`] writes the video.
pub enum RenderOutput {
    /// An exclusive temporary sibling of this destination file, which must not exist;
    /// [`publish_video`] links it into place.
    BesideDestination(PathBuf),
    /// This exact path in a folder the caller owns (a deck's staging folder). It must
    /// not exist; on failure nothing is left there.
    File(PathBuf),
}

/// A rendered video that the app decoded from start to end and flushed.
pub struct VerifiedVideo {
    path: PathBuf,
    /// The private temporary name of a [`RenderOutput::BesideDestination`] render and
    /// the canonical destination beside it.
    beside: Option<(tempfile::TempPath, PathBuf)>,
    revision: String,
    capabilities: Option<ExportCapabilities>,
}

impl VerifiedVideo {
    /// The verified file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The source revision it was rendered from.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// What the worker reported it can export, when it was asked (any export but the
    /// default one asks).
    pub fn capabilities(&self) -> Option<&ExportCapabilities> {
        self.capabilities.as_ref()
    }
}

/// Compiles and renders a frozen revision to an exclusive sibling temporary file.
/// A final destination is created only after the child closed and decoded the video.
pub fn export_video(
    request: ExportRequest,
    service: BuildService,
    processes: ProcessTreeManager,
    control: Arc<ExportControl>,
    mut progress: impl FnMut(ExportProgress),
) -> Result<(), String> {
    let ExportRequest {
        source,
        sdk,
        builds,
        destination,
        settings,
        timing_json,
        props_json,
    } = request;
    let verified = render_verified(
        RenderRequest {
            source,
            sdk,
            builds,
            settings,
            timing_json,
            props_json,
        },
        RenderOutput::BesideDestination(destination),
        &service,
        &processes,
        &control,
        &mut progress,
    )?;
    publish_video(verified, &processes, &control, &mut progress)
}

/// Compiles the frozen revision's worker, renders it to `output` and verifies the file
/// in the app process. Emits only non-terminal progress; on any failure or cancellation
/// the partial file is removed.
pub fn render_verified(
    request: RenderRequest,
    output: RenderOutput,
    service: &BuildService,
    processes: &ProcessTreeManager,
    control: &ExportControl,
    progress: &mut impl FnMut(ExportProgress),
) -> Result<VerifiedVideo, String> {
    let RenderRequest {
        source,
        sdk,
        builds,
        settings,
        timing_json,
        props_json,
    } = request;
    let revision = source.revision().as_str().to_owned();
    progress(ExportProgress::Started {
        revision: revision.clone(),
        label: source.label().to_owned(),
    });
    if let Some(warning) = source.live_source_warning() {
        progress(ExportProgress::Warning(format!(
            "Live project source was unavailable; using the intact published checkpoint: {warning}"
        )));
    }
    if processes.is_shutdown() || control.is_cancelled() {
        return Err("export cancelled before compilation".into());
    }

    let sdk_manifest = load_sdk_manifest(&sdk)?;

    let subscriber = Subscriber::new(
        SubscriberKind::Tool,
        format!("mp4-export-{}", uuid::Uuid::new_v4()),
    );
    let build = compile_portable_worker_via(
        service,
        subscriber,
        source.project(),
        &sdk,
        sdk_manifest,
        &builds,
        processes,
    )?;
    if processes.is_shutdown() || control.is_cancelled() {
        return Err("export cancelled after compilation".into());
    }
    let mut reported = None;
    if !settings.is_default() || timing_json.is_some() || props_json.is_some() {
        let capabilities = export_capabilities(&build, processes)?;
        capabilities
            .accepts(&settings)
            .map_err(|error| format!("cannot export with these settings: {error}"))?;
        if timing_json.is_some() && !capabilities.timing {
            return Err("this project's worker cannot export edited timing".into());
        }
        if props_json.is_some() && !capabilities.props {
            return Err(
                "this project's worker cannot export edited props; remove props.json or wire props hooks into its worker".into(),
            );
        }
        reported = Some(capabilities);
    }
    let timing_file = timing_json.as_deref().map(write_timing_file).transpose()?;
    let props_file = props_json.as_deref().map(write_props_file).transpose()?;

    let (temporary_path, beside) = match output {
        RenderOutput::BesideDestination(destination) => {
            let destination = prepare_destination(&destination, settings.container)?;
            if destination.exists() {
                return Err(format!(
                    "destination already exists; choose another file: {}",
                    destination.display()
                ));
            }
            let parent = destination
                .parent()
                .ok_or_else(|| "export destination has no parent directory".to_owned())?;
            let named_temporary = tempfile::Builder::new()
                .prefix(".fframes-export-")
                .suffix(&format!(".{}", settings.container.extension()))
                .tempfile_in(parent)
                .map_err(|error| format!("could not create export temporary file: {error}"))?;
            let temporary_path = named_temporary.path().to_owned();
            let temporary = named_temporary.into_temp_path();
            fs::remove_file(&temporary_path)
                .map_err(|error| format!("could not prepare export temporary file: {error}"))?;
            (temporary_path, Some((temporary, destination)))
        }
        RenderOutput::File(path) => {
            if fs::symlink_metadata(&path).is_ok() {
                return Err(format!("render output already exists: {}", path.display()));
            }
            (path, None)
        }
    };

    let mut options = worker_options(&build);
    options.args([EXPORT_FLAG, "--output"]);
    options.arg(&temporary_path);
    options.args(settings.to_args());
    options.args(crate::services::renderer_preference::worker_args());
    if let Some(timing_file) = &timing_file {
        options.arg("--timing-file");
        options.arg(timing_file);
    }
    if let Some(props_file) = &props_file {
        options.arg("--props-file");
        options.arg(props_file);
    }
    options.stdin(Stdio::null());
    options.stdout(Stdio::null());
    options.stderr(Stdio::piped());
    let child = processes
        .spawn(options)
        .map_err(|error| format!("could not start export renderer: {error}"))?;
    let stderr = child
        .lock()
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| "export renderer did not provide its progress stream".to_owned());
    let stderr = match stderr {
        Ok(stderr) => stderr,
        Err(error) => {
            let _ = child.lock().terminate_verified(Duration::from_millis(500));
            return Err(error);
        }
    };
    let (sender, receiver) = mpsc::sync_channel(16);
    let reader = thread::Builder::new()
        .name("studio-export-progress".into())
        .spawn(move || read_progress(stderr, sender));
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.lock().terminate_verified(Duration::from_millis(500));
            return Err(format!("could not start export progress reader: {error}"));
        }
    };
    let mut diagnostics = Vec::new();
    let status = loop {
        drain_progress(&receiver, &mut diagnostics, progress);
        if control.is_cancelled() || processes.is_shutdown() {
            let _ = child.lock().terminate_verified(Duration::from_millis(500));
            join_progress_reader(reader, &receiver, &mut diagnostics, progress);
            let _ = fs::remove_file(&temporary_path);
            return Err("export cancelled".into());
        }
        match child.lock().try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                let _ = child.lock().terminate_verified(Duration::from_millis(500));
                join_progress_reader(reader, &receiver, &mut diagnostics, progress);
                let _ = fs::remove_file(&temporary_path);
                return Err(format!("could not read export process status: {error}"));
            }
        }
    };
    join_progress_reader(reader, &receiver, &mut diagnostics, progress);
    if !status.success() {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "export renderer exited with {}{}",
            status
                .code()
                .map_or_else(|| "a signal".into(), |code| code.to_string()),
            diagnostics
                .last()
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ));
    }
    progress(ExportProgress::Verifying);
    let metadata = fs::metadata(&temporary_path)
        .map_err(|error| format!("export renderer did not produce a complete MP4: {error}"))?;
    if !metadata.is_file() || metadata.len() < 24 {
        let _ = fs::remove_file(&temporary_path);
        return Err("export output is empty or is not a complete MP4".into());
    }
    if let Err(error) = verify_export(&temporary_path, &settings) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    // Windows flushes only through a handle with write access.
    if let Err(error) = fs::OpenOptions::new()
        .write(true)
        .open(&temporary_path)
        .and_then(|file| file.sync_all())
    {
        if beside.is_none() {
            let _ = fs::remove_file(&temporary_path);
        }
        return Err(format!("could not flush MP4 before publication: {error}"));
    }
    Ok(VerifiedVideo {
        path: temporary_path,
        beside,
        revision,
        capabilities: reported,
    })
}

/// Links a video verified beside its destination into place without replacing anything,
/// then reports [`ExportProgress::Complete`].
pub fn publish_video(
    verified: VerifiedVideo,
    processes: &ProcessTreeManager,
    control: &ExportControl,
    progress: &mut impl FnMut(ExportProgress),
) -> Result<(), String> {
    let VerifiedVideo {
        path: temporary_path,
        beside,
        revision,
        ..
    } = verified;
    let Some((temporary, destination)) = beside else {
        return Err("this video was rendered into a folder and has no destination".into());
    };
    let parent = destination
        .parent()
        .ok_or_else(|| "export destination has no parent directory".to_owned())?;
    if processes.is_shutdown() || !control.begin_publication() {
        let _ = fs::remove_file(&temporary_path);
        return Err("export cancelled before publication".into());
    }
    match publish_no_clobber(&temporary_path, &destination) {
        Ok(()) => {
            control.finish_publication();
            if let Err(error) = temporary.close() {
                progress(ExportProgress::Warning(format!(
                    "MP4 was published, but its private temporary file could not be removed: {error}"
                )));
            }
            if let Err(error) = sync_directory(parent) {
                progress(ExportProgress::Warning(format!(
                    "MP4 was published, but destination metadata could not be flushed: {error}"
                )));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            control.abandon_publication();
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "destination appeared during export; existing bytes were preserved: {}",
                destination.display()
            ));
        }
        Err(error) => {
            control.abandon_publication();
            let _ = fs::remove_file(&temporary_path);
            return Err(format!("could not atomically publish MP4: {error}"));
        }
    }
    progress(ExportProgress::Complete {
        destination,
        revision,
    });
    Ok(())
}

fn worker_options(build: &MaterializedBuild) -> SpawnOptions {
    let mut options = SpawnOptions::new(worker_binary(build));
    options.current_dir(build.manifest.parent().unwrap_or(&build.root));
    options.env = build.environment.build_child_environment();
    options
}

fn load_sdk_manifest(sdk: &Path) -> Result<studio_sdk::CompatibilityManifest, String> {
    let path = sdk.join("compatibility.json");
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(&path)
            .map_err(|error| format!("SDK manifest {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("SDK manifest is invalid: {error}"))?;
    manifest
        .validate_for_current_app_version(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("SDK is not compatible with this app: {error}"))?;
    Ok(manifest)
}

/// What the portable worker of `source` can export. The worker is the one an export of the
/// same source compiles, so an unchanged project answers from the build the preview
/// already made.
pub fn probe_export_capabilities(
    source: &FrozenExportSource,
    sdk: &Path,
    builds: &Path,
    service: &BuildService,
    processes: &ProcessTreeManager,
) -> Result<ExportCapabilities, String> {
    let manifest = load_sdk_manifest(sdk)?;
    let subscriber = Subscriber::new(
        SubscriberKind::Tool,
        format!("export-capabilities-{}", uuid::Uuid::new_v4()),
    );
    let build = compile_portable_worker_via(
        service,
        subscriber,
        source.project(),
        sdk,
        manifest,
        builds,
        processes,
    )?;
    export_capabilities(&build, processes)
}

/// Asks the worker what it can export.
fn export_capabilities(
    build: &MaterializedBuild,
    processes: &ProcessTreeManager,
) -> Result<ExportCapabilities, String> {
    let mut options = worker_options(build);
    options.arg(EXPORT_CAPABILITIES_FLAG);
    options.stdin(Stdio::null());
    options.stdout(Stdio::piped());
    options.stderr(Stdio::null());
    let child = processes
        .spawn(options)
        .map_err(|error| format!("could not start the export renderer: {error}"))?;
    let stdout = child.lock().child_mut().stdout.take();
    let Some(stdout) = stdout else {
        let _ = child.lock().terminate_verified(Duration::from_millis(500));
        return Err("export renderer did not provide its capabilities stream".into());
    };
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.take(CAPABILITIES_LIMIT).read_to_end(&mut output);
        output
    });
    let started = Instant::now();
    let status = loop {
        match child.lock().try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < CAPABILITIES_TIMEOUT && !processes.is_shutdown() => {
                thread::sleep(POLL_INTERVAL)
            }
            Ok(None) => {
                let _ = child.lock().terminate_verified(Duration::from_millis(500));
                return Err("export renderer did not report its capabilities in time".into());
            }
            Err(error) => {
                let _ = child.lock().terminate_verified(Duration::from_millis(500));
                return Err(format!("could not read export renderer status: {error}"));
            }
        }
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!(
            "export renderer could not report its capabilities ({status})"
        ));
    }
    let capabilities: ExportCapabilities = serde_json::from_slice(&output)
        .map_err(|error| format!("export renderer reported unreadable capabilities: {error}"))?;
    if capabilities.version != EXPORT_CAPABILITIES_VERSION {
        return Err(format!(
            "export renderer reports capabilities version {}; this app reads {}",
            capabilities.version, EXPORT_CAPABILITIES_VERSION
        ));
    }
    Ok(capabilities)
}

/// Writes the timing document for the worker to read; it is removed when dropped.
fn write_timing_file(json: &str) -> Result<tempfile::TempPath, String> {
    write_document_file(json, "timing", MAX_TIMING_DOC_BYTES)
}

/// Writes the props document for the worker to read; it is removed when dropped.
fn write_props_file(json: &str) -> Result<tempfile::TempPath, String> {
    write_document_file(json, "props", fframes_studio_protocol::MAX_PROPS_DOC_BYTES)
}

fn write_document_file(
    json: &str,
    kind: &str,
    max_bytes: usize,
) -> Result<tempfile::TempPath, String> {
    if json.len() > max_bytes {
        return Err(format!(
            "{kind} document is {} bytes; the limit is {max_bytes}",
            json.len()
        ));
    }
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".fframes-{kind}-"))
        .suffix(".json")
        .tempfile()
        .map_err(|error| format!("could not create the export {kind} file: {error}"))?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| format!("could not write the export {kind} file: {error}"))?;
    Ok(file.into_temp_path())
}

fn verify_export(path: &Path, settings: &ExportSettings) -> Result<(), String> {
    // Verify in the app process: a successful worker exit and its own decoder check
    // are not sufficient evidence for publishing the output.
    let streams = probe_streams(path)
        .map_err(|error| format!("export output is not a decodable video: {error:?}"))?;
    let codec = settings.codec.stream_codec_name();
    if streams.video_codec.as_deref() != Some(codec) {
        return Err(format!(
            "export output holds {} video instead of {codec}",
            streams.video_codec.as_deref().unwrap_or("no")
        ));
    }
    let mut video = unsafe { FFmpegDecoder::new(path, VERIFY_FRAME_RATE, 1) }
        .map_err(|error| format!("export output is not a decodable video: {error:?}"))?;
    let decoded_video = unsafe { video.decode_up_to(0) }
        .map_err(|error| format!("export video stream could not be decoded: {error:?}"))?;
    if !decoded_video {
        return Err("export MP4 contains no decodable video frame".into());
    }
    if video.get_stream_width() == 0 || video.get_stream_height() == 0 {
        return Err("export MP4 has invalid video dimensions".into());
    }
    let Some(last_frame_offset) = video.get_last_frame_offset() else {
        return Err("export MP4 does not report a usable video duration".into());
    };
    decode_video_through_offset(&mut video, last_frame_offset / 2, "midpoint")?;
    decode_video_through_offset(&mut video, last_frame_offset, "final frame")?;
    if video.has_audio_stream() {
        let mut audio = AudioDecoder::new(path, None)
            .map_err(|error| format!("export audio stream is invalid: {error:?}"))?;
        let (_, channels) = audio
            .decode_preview_samples(4096)
            .map_err(|error| format!("export audio stream could not be decoded: {error:?}"))?;
        if channels.is_empty() || channels[0].is_empty() {
            return Err("export MP4 contains no decodable audio samples".into());
        }
    }
    Ok(())
}

fn decode_video_through_offset(
    video: &mut FFmpegDecoder,
    offset: i64,
    description: &str,
) -> Result<(), String> {
    let decoded = unsafe { video.decode_up_to(offset) }
        .map_err(|error| format!("export video {description} could not be decoded: {error:?}"))?;
    if !decoded {
        return Err(format!(
            "export MP4 contains no decodable {description} video frame"
        ));
    }

    // `decode_up_to` may return the last available frame when demuxing reaches EOF. That is
    // useful when displaying a short stream, but must not let a truncated export pass as if
    // it reached its reported midpoint or tail. Allow one verification tick for frame-rate
    // rounding while requiring the decoded frame to be near the requested position.
    let requested_seconds = offset as f64 / VERIFY_FRAME_RATE as f64;
    let decoded_seconds = f64::from(unsafe { video.get_raw_frame().timestamp_seconds() });
    let one_frame_seconds = 1.0 / VERIFY_FRAME_RATE as f64;
    if !decoded_seconds.is_finite() || decoded_seconds + one_frame_seconds < requested_seconds {
        return Err(format!(
            "export video ended before its requested {description} frame"
        ));
    }
    Ok(())
}

fn join_progress_reader(
    reader: JoinHandle<()>,
    receiver: &Receiver<String>,
    diagnostics: &mut Vec<String>,
    progress: &mut impl FnMut(ExportProgress),
) {
    while !reader.is_finished() {
        drain_progress(receiver, diagnostics, progress);
        thread::sleep(Duration::from_millis(2));
    }
    let _ = reader.join();
    drain_progress(receiver, diagnostics, progress);
}

fn prepare_destination(path: &Path, container: ExportContainer) -> Result<PathBuf, String> {
    let extension = container.extension();
    if path.extension().and_then(|value| value.to_str()) != Some(extension) {
        return Err(format!("choose a destination ending in .{extension}"));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()
        .map_err(|error| format!("export destination folder is unavailable: {error}"))?;
    let filename = path
        .file_name()
        .ok_or_else(|| "export destination has no file name".to_owned())?;
    Ok(parent.join(filename))
}

fn publish_no_clobber(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination)
}

fn read_progress(mut stderr: impl Read, sender: SyncSender<String>) {
    let mut chunk = [0u8; 4096];
    let mut line = Vec::new();
    let mut overlong = false;
    loop {
        let count = match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if overlong {
                    let _ = sender.send("export progress line exceeded its size limit".into());
                } else if !line.is_empty() {
                    let _ = sender.send(String::from_utf8_lossy(&line).into_owned());
                }
                line.clear();
                overlong = false;
            } else if line.len() < PROGRESS_LINE_LIMIT {
                line.push(*byte);
            } else {
                overlong = true;
            }
        }
    }
    if !line.is_empty() {
        let _ = sender.send(if overlong {
            "export progress line exceeded its size limit".into()
        } else {
            String::from_utf8_lossy(&line).into_owned()
        });
    }
}

fn drain_progress(
    receiver: &Receiver<String>,
    diagnostics: &mut Vec<String>,
    progress: &mut impl FnMut(ExportProgress),
) {
    while let Ok(line) = receiver.try_recv() {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&line);
        let Ok(event) = parsed else {
            push_diagnostic(diagnostics, line);
            continue;
        };
        match event.get("event").and_then(serde_json::Value::as_str) {
            Some("progress") => {
                if let (Some(done), Some(total)) = (
                    event.get("done").and_then(serde_json::Value::as_u64),
                    event.get("total").and_then(serde_json::Value::as_u64),
                ) {
                    progress(ExportProgress::Rendering {
                        done: done.min(usize::MAX as u64) as usize,
                        total: total.min(usize::MAX as u64) as usize,
                    });
                }
            }
            Some("audio") => progress(ExportProgress::Audio),
            Some("warning") => {
                if let Some(message) = event.get("message").and_then(serde_json::Value::as_str) {
                    progress(ExportProgress::Warning(message.to_owned()));
                }
            }
            _ => push_diagnostic(diagnostics, line),
        }
    }
}

fn push_diagnostic(diagnostics: &mut Vec<String>, line: String) {
    if diagnostics.len() == DIAGNOSTIC_LIMIT {
        diagnostics.remove(0);
    }
    diagnostics.push(line);
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn export_cancel_and_publication_are_serialized_by_the_same_fence() {
        let control = ExportControl::default();
        assert!(control.request_cancel());
        assert!(control.is_cancelled());
        assert!(!control.begin_publication());

        let control = ExportControl::default();
        assert!(control.begin_publication());
        assert!(control.is_publishing());
        assert!(!control.request_cancel());
        control.finish_publication();
        assert!(!control.is_cancelled());
        assert!(!control.request_cancel());
    }

    #[test]
    fn export_destination_must_match_the_container_in_an_existing_parent() {
        let temporary = tempfile::tempdir().unwrap();
        let mp4 = ExportContainer::Mp4;
        let target = temporary.path().join("movie.mp4");
        let expected = temporary.path().canonicalize().unwrap().join("movie.mp4");
        assert_eq!(prepare_destination(&target, mp4).unwrap(), expected);
        assert!(prepare_destination(&temporary.path().join("movie.mov"), mp4).is_err());
        assert!(prepare_destination(&temporary.path().join("missing/movie.mp4"), mp4).is_err());
        assert!(prepare_destination(&target, ExportContainer::Webm).is_err());
        let webm = temporary.path().join("movie.webm");
        assert!(prepare_destination(&webm, ExportContainer::Webm).is_ok());
    }

    #[test]
    fn invalid_mp4_bytes_are_rejected_before_publication() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("invalid.mp4");
        fs::write(
            &path,
            b"this is not an mp4 container, despite being long enough",
        )
        .unwrap();

        assert!(
            verify_export(&path, &ExportSettings::default())
                .unwrap_err()
                .contains("not a decodable video")
        );
        assert!(!temporary.path().join("published.mp4").exists());
    }

    #[test]
    fn app_verifier_decodes_the_first_and_final_video_frames() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("complete.mp4");
        let generated = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=32x32:r=10:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100:duration=1",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-shortest",
                "-movflags",
                "+faststart",
                "-y",
            ])
            .arg(&path)
            .output();
        let Ok(generated) = generated else {
            eprintln!("skipping generated MP4 verification: ffmpeg CLI is unavailable");
            return;
        };
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        assert!(
            unsafe { FFmpegDecoder::new(&path, 30, 1) }
                .unwrap()
                .has_audio_stream()
        );
        verify_export(&path, &ExportSettings::default()).unwrap();

        let length = fs::metadata(&path).unwrap().len();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length / 2)
            .unwrap();
        let error = verify_export(&path, &ExportSettings::default()).unwrap_err();
        assert!(
            error.contains("ended before its requested"),
            "truncated MP4 must fail its frame-position check, got: {error}"
        );
    }

    #[test]
    fn exclusive_publication_preserves_existing_destination_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("complete.mp4");
        let destination = temporary.path().join("chosen.mp4");
        fs::write(&source, b"verified output").unwrap();
        fs::write(&destination, b"user file").unwrap();

        let error = publish_no_clobber(&source, &destination).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(destination).unwrap(), b"user file");
        assert_eq!(fs::read(source).unwrap(), b"verified output");
    }

    #[test]
    fn progress_reader_bounds_lines_and_preserves_multiple_events() {
        let mut input = b"{\"event\":\"progress\",\"done\":1,\"total\":2}\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', PROGRESS_LINE_LIMIT + 1));
        input.push(b'\n');
        let (sender, receiver) = mpsc::sync_channel(4);
        read_progress(input.as_slice(), sender);
        let events: Vec<_> = receiver.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("progress"));
        assert_eq!(events[1], "export progress line exceeded its size limit");
    }
}
