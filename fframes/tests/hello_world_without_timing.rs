//! Frames of a video without a timing document must stay exactly what they were before
//! timing documents existed. The snapshots were approved from the renderer before the
//! timing hook was added; they match byte for byte on the machine that approved them
//! (Windows), and within the default anti-aliasing tolerance on other platforms.
//!
//! The video is `examples/hello-world`, included by path: the example crate itself
//! enables GPL encoders whose `FFmpeg` build is not available on every test machine.
#[path = "../../examples/hello-world/src/hello_world.rs"]
#[allow(dead_code)]
mod hello_world;

use fframes::{CpuFrameRenderer, Previewer, RenderOptions, StaticMediaProvider};
use hello_world::{HelloWorldMedia, HelloWorldVideo};

#[test]
fn hello_world_frames_without_timing_are_unchanged() {
    let media = HelloWorldMedia::prepare().unwrap();
    let video = HelloWorldVideo {
        media: &media,
        slug: "Renderer!",
    };
    let options = RenderOptions {
        media: Some(&media),
        ..Default::default()
    };
    let mut previewer = Previewer::new(&video, &options).unwrap();
    let mut renderer = CpuFrameRenderer::default();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../examples/hello-world/_frame_snapshots");
    // A pixel differs when a channel differs by more than the threshold; the frame fails when
    // more than `max_diff_ratio` of its pixels differ.
    let (channel_threshold, max_diff_ratio) = if cfg!(windows) { (0, 0.) } else { (16, 0.001) };

    for spec in ["0", "0.5s", "15s", "end"] {
        let frame = previewer.timeline().resolve_frame(spec).unwrap();
        let actual = previewer.render(frame, &mut renderer).unwrap();
        let expected = image::open(directory.join(format!("{spec}.png")))
            .unwrap()
            .into_rgba8();
        assert_eq!(
            (expected.width(), expected.height()),
            (actual.width, actual.height),
            "{spec}: size differs"
        );
        let differing = expected
            .as_raw()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(actual.pixels.as_chunks::<4>().0)
            .filter(|(e, a)| {
                e.iter()
                    .zip(*a)
                    .any(|(e, a)| e.abs_diff(*a) > channel_threshold)
            })
            .count();
        let ratio = differing as f64 / f64::from(actual.width * actual.height);
        assert!(
            ratio <= max_diff_ratio,
            "{spec}: {:.3}% of pixels differ",
            ratio * 100.
        );
    }
}
