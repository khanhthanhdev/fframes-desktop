//! The slide deck export: a folder that plays the project as a slide deck in a browser,
//! straight from disk.
//!
//! ```text
//! <name>/
//! ├─ index.html    stage, overview grid, notes panel; the manifest inlined as JSON
//! ├─ style.css     theme and print stylesheet (one poster per page)
//! ├─ app.js        navigation, exact stops, overview, notes, URL hash, full screen
//! ├─ deck.mp4      H.264 + AAC with faststart, the same render as Video export
//! ├─ slides.json   the same manifest, for tools
//! ├─ posters/      0001.jpg … full size, one per stop
//! └─ thumbs/       0001.jpg … 320 px wide, for the overview grid
//! ```
//!
//! [`export_deck`] renders through [`render_verified`] into a staging folder next to the
//! destination, decodes the posters from the verified video, writes the page and
//! publishes the folder with one no-replace rename. One [`ExportControl`] covers the
//! whole job: Cancel works until the rename, and `Complete` is sent only after it.
pub mod manifest;
pub mod posters;
pub mod publish;

use std::{fs, path::PathBuf};

use fframes_studio_protocol::export::{ExportCodec, ExportContainer};
use studio_bootstrap::ProcessTreeManager;

pub use manifest::{DeckStop, Manifest};
use publish::{Destination, Staging};

use crate::services::{
    build_service::BuildService,
    export_service::{ExportControl, ExportProgress, RenderOutput, RenderRequest, render_verified},
};

/// The video file in the deck folder.
pub const DECK_VIDEO: &str = "deck.mp4";

/// A deck export of a frozen source.
pub struct DeckRequest {
    /// What to render; the settings must be MP4/H.264 at the video's frame rate.
    pub render: RenderRequest,
    /// The folder to create; it must not exist.
    pub destination: PathBuf,
    pub title: String,
    /// The resolved stops: sorted, after frame 0, the last on the video's last frame.
    pub stops: Vec<DeckStop>,
}

/// Renders, decodes, writes and publishes a deck. Emits non-terminal progress and, after
/// the folder was published, `Complete`; the caller reports a returned error. On any
/// error or cancellation the staging folder is removed.
pub fn export_deck(
    request: DeckRequest,
    service: &BuildService,
    processes: &ProcessTreeManager,
    control: &ExportControl,
    progress: &mut impl FnMut(ExportProgress),
) -> Result<(), String> {
    let DeckRequest {
        render,
        destination,
        title,
        stops,
    } = request;
    let settings = render.settings;
    if settings.container != ExportContainer::Mp4 || settings.codec != ExportCodec::H264 {
        return Err("a slide deck is exported as H.264 in MP4".into());
    }
    if settings.fps.is_some() {
        return Err("a slide deck plays at the video's own frame rate".into());
    }
    manifest::check_stops(&stops)?;
    let destination = Destination::prepare(&destination)?;
    publish::sweep_orphans(destination.parent());
    let staging = Staging::create(destination.parent())?;
    let verified = render_verified(
        render,
        RenderOutput::File(staging.path().join(DECK_VIDEO)),
        service,
        processes,
        control,
        progress,
    )?;
    let fps = verified
        .capabilities()
        .map(|capabilities| capabilities.fps)
        .ok_or_else(|| "the project's worker did not report its frame rate".to_owned())?;
    let revision = verified.revision().to_owned();
    drop(verified);
    finish(
        staging,
        &destination,
        Finish {
            title: &title,
            fps,
            stops: &stops,
            revision,
        },
        &|| processes.is_shutdown() || control.is_cancelled(),
        control,
        progress,
    )
}

/// What [`finish`] needs besides the folder.
struct Finish<'a> {
    title: &'a str,
    fps: u32,
    stops: &'a [DeckStop],
    revision: String,
}

/// Posters, page, publication: everything after the verified `deck.mp4` is in `staging`.
fn finish(
    mut staging: Staging,
    destination: &Destination,
    deck: Finish<'_>,
    cancelled: &dyn Fn() -> bool,
    control: &ExportControl,
    progress: &mut impl FnMut(ExportProgress),
) -> Result<(), String> {
    let folder = staging.path();
    let frames: Vec<usize> = deck.stops.iter().map(|stop| stop.video_frame).collect();
    let size = posters::extract(
        &folder.join(DECK_VIDEO),
        deck.fps,
        &frames,
        &folder,
        cancelled,
        &mut |event| progress(event),
    )?;
    if cancelled() {
        return Err("export cancelled".into());
    }
    progress(ExportProgress::WritingDeck);
    let manifest = Manifest::new(deck.title, (size.width, size.height), deck.fps, deck.stops)?;
    let json = manifest.to_inline_json();
    for (name, contents) in [
        ("slides.json", json.clone()),
        ("index.html", manifest::render_index(&json)),
        ("style.css", manifest::STYLE_CSS.to_owned()),
        ("app.js", manifest::APP_JS.to_owned()),
    ] {
        fs::write(folder.join(name), contents)
            .map_err(|error| format!("could not write the deck's {name}: {error}"))?;
    }
    if cancelled() || !control.begin_publication() {
        return Err("export cancelled before publication".into());
    }
    match staging.publish(destination) {
        Ok(()) => control.finish_publication(),
        Err(error) => {
            control.abandon_publication();
            return Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "{} appeared during the export and was left untouched; choose a new folder name",
                    destination.path().display()
                )
            } else {
                format!("could not publish the deck folder: {error}")
            });
        }
    }
    progress(ExportProgress::Complete {
        destination: destination.path(),
        revision: deck.revision,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, process::Command};

    /// A short H.264 MP4 with audio, or `None` without the ffmpeg CLI.
    fn sample_video(path: &Path, frames: usize) -> Option<()> {
        let output = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("testsrc2=s=64x36:r=30:d={}", frames as f64 / 30.),
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-movflags",
                "+faststart",
                "-y",
            ])
            .arg(path)
            .output()
            .ok()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(())
    }

    fn stops(frames: &[usize]) -> Vec<DeckStop> {
        frames
            .iter()
            .map(|&video_frame| DeckStop {
                video_frame,
                title: Some(format!("Stop {video_frame}")),
                notes: Some("<b>notes</b>".into()),
            })
            .collect()
    }

    fn staged(parent: &Path, frames: usize) -> Option<Staging> {
        let staging = Staging::create(parent).unwrap();
        sample_video(&staging.path().join(DECK_VIDEO), frames)?;
        Some(staging)
    }

    #[test]
    fn a_finished_deck_is_published_with_every_file() {
        let temp = tempfile::tempdir().unwrap();
        let destination = Destination::prepare(&temp.path().join("Slides")).unwrap();
        let Some(staging) = staged(destination.parent(), 30) else {
            eprintln!("skipping: ffmpeg CLI is unavailable");
            return;
        };
        let control = ExportControl::default();
        let mut events = Vec::new();
        let stops = stops(&[10, 20, 29]);
        finish(
            staging,
            &destination,
            Finish {
                title: "Deck",
                fps: 30,
                stops: &stops,
                revision: "r".into(),
            },
            &|| false,
            &control,
            &mut |event| events.push(event),
        )
        .unwrap();
        let folder = destination.path();
        for name in [
            "index.html",
            "style.css",
            "app.js",
            "slides.json",
            DECK_VIDEO,
            "posters/0001.jpg",
            "posters/0003.jpg",
            "thumbs/0003.jpg",
        ] {
            assert!(folder.join(name).is_file(), "{name}");
        }
        assert!(!folder.join(publish::OWNER_FILE).exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(folder.join("slides.json")).unwrap()).unwrap();
        assert_eq!(manifest["total_frames"], 30);
        assert_eq!(manifest["width"], 64);
        assert_eq!(manifest["slides"][2]["start_frame"], 20);
        assert_eq!(manifest["slides"][0]["notes"], "<b>notes</b>");
        let poster = image::open(folder.join("posters/0001.jpg")).unwrap();
        assert_eq!((poster.width(), poster.height()), (64, 36));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ExportProgress::Posters { .. }))
                .count(),
            3
        );
        assert!(matches!(
            events.last(),
            Some(ExportProgress::Complete { .. })
        ));
        assert!(!control.request_cancel(), "published: cancel is too late");
    }

    #[test]
    fn cancel_during_posters_stops_and_deletes_the_staging_folder() {
        let temp = tempfile::tempdir().unwrap();
        let destination = Destination::prepare(&temp.path().join("Slides")).unwrap();
        let Some(staging) = staged(destination.parent(), 30) else {
            eprintln!("skipping: ffmpeg CLI is unavailable");
            return;
        };
        let staged_path = staging.path();
        let control = ExportControl::default();
        let mut events = Vec::new();
        let stops = stops(&[10, 20, 29]);
        let error = finish(
            staging,
            &destination,
            Finish {
                title: "Deck",
                fps: 30,
                stops: &stops,
                revision: "r".into(),
            },
            &|| control.is_cancelled(),
            &control,
            &mut |event| {
                if matches!(event, ExportProgress::Posters { done: 1, .. }) {
                    assert!(control.request_cancel());
                }
                events.push(event);
            },
        )
        .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        let posters = events
            .iter()
            .filter(|event| matches!(event, ExportProgress::Posters { .. }))
            .count();
        assert_eq!(posters, 1, "no frame after the cancel");
        assert!(!staged_path.exists(), "staging removed");
        assert!(!destination.path().exists(), "nothing published");
        assert!(control.is_cancelled());
    }

    #[test]
    fn cancel_during_the_initial_decode_prefix_publishes_no_posters() {
        let temp = tempfile::tempdir().unwrap();
        let destination = Destination::prepare(&temp.path().join("Slides")).unwrap();
        let Some(staging) = staged(destination.parent(), 120) else {
            eprintln!("skipping: ffmpeg CLI is unavailable");
            return;
        };
        let staged_path = staging.path();
        let polls = std::cell::Cell::new(0);
        let mut events = Vec::new();
        let error = finish(
            staging,
            &destination,
            Finish {
                title: "Deck",
                fps: 30,
                stops: &stops(&[119]),
                revision: "r".into(),
            },
            &|| {
                polls.set(polls.get() + 1);
                polls.get() >= 4
            },
            &ExportControl::default(),
            &mut |event| events.push(event),
        )
        .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert!(
            events.is_empty(),
            "cancelled before the first poster: {events:?}"
        );
        assert!(!staged_path.exists(), "staging removed");
        assert!(!destination.path().exists(), "nothing published");
    }

    #[test]
    fn a_video_of_another_length_than_the_stops_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let destination = Destination::prepare(&temp.path().join("Slides")).unwrap();
        let Some(staging) = staged(destination.parent(), 60) else {
            eprintln!("skipping: ffmpeg CLI is unavailable");
            return;
        };
        let stops = stops(&[10, 29]);
        let error = finish(
            staging,
            &destination,
            Finish {
                title: "Deck",
                fps: 30,
                stops: &stops,
                revision: "r".into(),
            },
            &|| false,
            &ExportControl::default(),
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.contains("wait for the preview"), "{error}");
        assert!(!destination.path().exists());
    }
}
