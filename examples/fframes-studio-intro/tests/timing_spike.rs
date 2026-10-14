//! Spike: a timing document reorders, trims and splits the intro's code scenes in the
//! previewer, and the render path given the same document draws the same frames.
use fframes::{
    CpuFrameRenderer, Previewer, RenderOptions, StaticMediaProvider, TimingClip, TimingDoc, Video,
};
use fframes_studio_intro::{StudioIntroMedia, StudioIntroVideo};

#[test]
fn timing_reorders_trims_and_splits_the_intro() {
    let embedded = StudioIntroMedia::prepare().expect("media");
    let folder =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../fframes-intro/dynamic_media");
    let dir = fframes::MediaDirectory::read_folder(&folder).expect("dynamic media");
    let dynamic = dir.process_media_source().expect("dynamic media");
    let media =
        fframes::CombinedMediaProvider::from([&embedded as &dyn fframes::MediaProvider, &dynamic]);
    let video = StudioIntroVideo;
    let base = RenderOptions {
        media: Some(&media),
        scale_resolution: 0.25,
        ..Default::default()
    };
    let mut previewer = Previewer::new(&video, &base).unwrap();
    let code: Vec<_> = previewer.timeline().scenes.clone();
    let keys: Vec<String> = code.iter().map(|s| s.timing_key.clone()).collect();
    println!(
        "code order: {} frames",
        previewer.timeline().duration_in_frames
    );
    for s in &code {
        println!("  {} {:?}", s.timing_key, s.frames);
    }
    let length = |i: usize| code[i].frames.len();

    // The last scene first, the first scene without its first 20 frames, then the
    // second scene split around a 20 frame gap; the other scenes are appended.
    let last = keys.len() - 1;
    let half = length(1) / 2;
    let doc = TimingDoc {
        version: fframes::TIMING_DOC_VERSION,
        clips: vec![
            TimingClip {
                key: keys[last].clone(),
                trim_start_frames: 0,
                trim_end_frames: 0,
            },
            TimingClip {
                key: keys[0].clone(),
                trim_start_frames: 20,
                trim_end_frames: 0,
            },
            TimingClip {
                key: keys[1].clone(),
                trim_start_frames: 0,
                trim_end_frames: length(1) - half,
            },
            TimingClip {
                key: keys[1].clone(),
                trim_start_frames: half + 20,
                trim_end_frames: 0,
            },
        ],
    };
    let mut renderer = CpuFrameRenderer::default();
    let before = previewer.render(5, &mut renderer).unwrap().pixels;
    let report = previewer.set_timing(Some(&doc)).unwrap();
    assert!(report.orphan_clips.is_empty());
    assert_eq!(report.appended_scenes.len(), keys.len() - 3);
    println!("timed: {} frames", previewer.timeline().duration_in_frames);
    for s in &previewer.timeline().scenes {
        println!(
            "  {} #{} {:?} trims {}+{}",
            s.timing_key, s.clip_index, s.frames, s.trim_start, s.trim_end
        );
    }
    let after = previewer.render(5, &mut renderer).unwrap().pixels;
    assert_ne!(before, after, "frame 5 now shows the last scene");

    let timed = RenderOptions {
        timing: Some(&doc),
        ..base.clone()
    };
    let scenes = previewer.timeline().scenes.clone();
    // Inside the moved last scene, the trimmed first scene and the second half of the split.
    let frames = [5, scenes[1].frames.start + 3, scenes[3].frames.start + 3];
    for frame in frames {
        let previewed = previewer.render(frame, &mut renderer).unwrap().pixels;
        let rendered = fframes::render_frame(
            frame,
            &video,
            fframes::cpu::CpuRenderingBackend::default(),
            &timed,
        )
        .unwrap();
        let differing = rendered
            .iter()
            .zip(&previewed)
            .filter(|(a, b)| a != b)
            .count();
        println!("frame {frame}: {differing} bytes differ");
        assert_eq!(differing, 0, "frame {frame}");
    }
    assert!(StudioIntroVideo::FPS > 0);
}
