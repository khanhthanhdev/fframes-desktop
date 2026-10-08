use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use fframes_media::{AudioDecoder, FFmpegDecoder};
use studio_bootstrap::{ProcessTreeManager, SpawnOptions};
use studio_engine::FrozenExportSource;

use crate::{
    build_service::{BuildService, Subscriber, SubscriberKind},
    worker_project::{compile_portable_worker_via, worker_binary},
};

const PROGRESS_LINE_LIMIT: usize = 64 * 1024;
const DIAGNOSTIC_LIMIT: usize = 32;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
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

    fn begin_publication(&self) -> bool {
        self.state
            .compare_exchange(
                EXPORT_ACTIVE,
                EXPORT_PUBLISHING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn finish_publication(&self) {
        self.state.store(EXPORT_PUBLISHED, Ordering::Release);
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
}

/// Compiles and renders a frozen revision to an exclusive sibling temporary file.
/// A final destination is created only after the child closed and decoded the MP4.
pub fn export_mp4(
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

    let sdk_manifest_path = sdk.join("compatibility.json");
    let sdk_manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(&sdk_manifest_path)
            .map_err(|error| format!("SDK manifest {}: {error}", sdk_manifest_path.display()))?,
    )
    .map_err(|error| format!("SDK manifest is invalid: {error}"))?;
    sdk_manifest
        .validate_for_current_app_version(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("SDK is not compatible with this app: {error}"))?;

    let subscriber = Subscriber::new(
        SubscriberKind::Tool,
        format!("mp4-export-{}", uuid::Uuid::new_v4()),
    );
    let build = compile_portable_worker_via(
        &service,
        subscriber,
        source.project(),
        &sdk,
        sdk_manifest,
        &builds,
        &processes,
    )?;
    if processes.is_shutdown() || control.is_cancelled() {
        return Err("export cancelled after compilation".into());
    }

    let destination = prepare_destination(&destination)?;
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
        .suffix(".mp4")
        .tempfile_in(parent)
        .map_err(|error| format!("could not create export temporary file: {error}"))?;
    let temporary_path = named_temporary.path().to_owned();
    let temporary = named_temporary.into_temp_path();
    fs::remove_file(&temporary_path)
        .map_err(|error| format!("could not prepare export temporary file: {error}"))?;

    let mut options = SpawnOptions::new(worker_binary(&build));
    options.args(["--export-v1", "--output"]);
    options.arg(&temporary_path);
    options.current_dir(build.manifest.parent().unwrap_or(&build.root));
    options.env = build.environment.build_child_environment();
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
        drain_progress(&receiver, &mut diagnostics, &mut progress);
        if control.is_cancelled() || processes.is_shutdown() {
            let _ = child.lock().terminate_verified(Duration::from_millis(500));
            join_progress_reader(reader, &receiver, &mut diagnostics, &mut progress);
            let _ = fs::remove_file(&temporary_path);
            return Err("export cancelled".into());
        }
        match child.lock().try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                let _ = child.lock().terminate_verified(Duration::from_millis(500));
                join_progress_reader(reader, &receiver, &mut diagnostics, &mut progress);
                let _ = fs::remove_file(&temporary_path);
                return Err(format!("could not read export process status: {error}"));
            }
        }
    };
    join_progress_reader(reader, &receiver, &mut diagnostics, &mut progress);
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
    if let Err(error) = verify_mp4(&temporary_path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    File::open(&temporary_path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("could not flush MP4 before publication: {error}"))?;
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
            control.state.store(EXPORT_ACTIVE, Ordering::Release);
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "destination appeared during export; existing bytes were preserved: {}",
                destination.display()
            ));
        }
        Err(error) => {
            control.state.store(EXPORT_ACTIVE, Ordering::Release);
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

fn verify_mp4(path: &Path) -> Result<(), String> {
    // Verify in the app process: a successful worker exit and its own decoder check
    // are not sufficient evidence for publishing the output.
    let mut video = unsafe { FFmpegDecoder::new(path, 30, 1) }
        .map_err(|error| format!("export output is not a decodable MP4: {error:?}"))?;
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
    let decoded_middle_frame = unsafe { video.decode_up_to(last_frame_offset / 2) }
        .map_err(|error| format!("export video midpoint could not be decoded: {error:?}"))?;
    if !decoded_middle_frame {
        return Err("export MP4 contains no decodable middle video frame".into());
    }
    let decoded_last_frame = unsafe { video.decode_up_to(last_frame_offset) }
        .map_err(|error| format!("export video tail could not be decoded: {error:?}"))?;
    if !decoded_last_frame {
        return Err("export MP4 contains no decodable final video frame".into());
    }
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

fn prepare_destination(path: &Path) -> Result<PathBuf, String> {
    if path.extension().and_then(|value| value.to_str()) != Some("mp4") {
        return Err("choose a destination ending in .mp4".into());
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
    File::open(path)?.sync_all()?;
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
    fn export_destination_must_be_mp4_in_an_existing_parent() {
        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("movie.mp4");
        let expected = temporary.path().canonicalize().unwrap().join("movie.mp4");
        assert_eq!(prepare_destination(&target).unwrap(), expected);
        assert!(prepare_destination(&temporary.path().join("movie.mov")).is_err());
        assert!(prepare_destination(&temporary.path().join("missing/movie.mp4")).is_err());
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
            verify_mp4(&path)
                .unwrap_err()
                .contains("not a decodable MP4")
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
        verify_mp4(&path).unwrap();

        let length = fs::metadata(&path).unwrap().len();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(length / 2)
            .unwrap();
        assert!(verify_mp4(&path).is_err(), "truncated MP4 must not verify");
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
