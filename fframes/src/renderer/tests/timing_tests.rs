use crate::{
    AudioMap, AudioTimelineUnit, AudioTimestamp, Color, CpuFrameRenderer, FFramesContext, Frame,
    Overlap, Previewer, RenderOptions, Scene, Svgr, TimingClip, TimingDoc, Video,
};

/// Paints one pixel: red = scene id, green = the scene frame it was asked for,
/// blue = `get_scene_info(self).start_frame`.
#[derive(Debug)]
struct Probe {
    id: u8,
    key: Option<&'static str>,
    overlap_previous: bool,
    audio: bool,
}

impl Scene for Probe {
    fn duration(&self) -> crate::Duration<'_> {
        crate::Duration::Frames(10)
    }

    fn overlap(&self) -> Overlap {
        if self.overlap_previous {
            Overlap::Previous(0.2)
        } else {
            Overlap::None
        }
    }

    fn audio(&self) -> AudioMap<'_> {
        if self.audio {
            AudioMap::from([(
                "voice.wav",
                AudioTimestamp::Second(0.)..AudioTimestamp::Second(1.),
            )])
        } else {
            AudioMap::none()
        }
    }

    fn editor_instance_key(&self) -> Option<&str> {
        self.key
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let start = ctx
            .get_scene_info(self)
            .map_or(255, |info| info.start_frame);
        let fill = Color::rgb(self.id * 50, frame.index as u8, start as u8);
        crate::svgr!(<rect x="0" y="0" width="2" height="2" fill={fill} />)
    }
}

struct ThreeScenes {
    a: Probe,
    b: Probe,
    c: Probe,
}

impl ThreeScenes {
    fn new() -> Self {
        Self {
            a: Probe {
                id: 1,
                key: Some("a"),
                overlap_previous: false,
                audio: false,
            },
            b: Probe {
                id: 2,
                key: Some("b"),
                overlap_previous: true,
                audio: false,
            },
            c: Probe {
                id: 3,
                key: None,
                overlap_previous: false,
                audio: true,
            },
        }
    }
}

impl Video for ThreeScenes {
    const FPS: usize = 10;
    const WIDTH: usize = 2;
    const HEIGHT: usize = 2;

    fn duration(&self) -> crate::Duration<'_> {
        crate::Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn define_scenes(&self) -> crate::Scenes<'_> {
        crate::Scenes::from(vec![&self.a as &dyn Scene, &self.b, &self.c])
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        crate::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">{ctx.render_scenes(&frame)}</svg>
        )
    }
}

fn c_key() -> String {
    std::any::type_name::<Probe>().to_owned()
}

fn doc(clips: &[(&str, usize, usize)]) -> TimingDoc {
    TimingDoc {
        version: crate::TIMING_DOC_VERSION,
        clips: clips
            .iter()
            .map(|(key, trim_start_frames, trim_end_frames)| TimingClip {
                key: (*key).to_owned(),
                trim_start_frames: *trim_start_frames,
                trim_end_frames: *trim_end_frames,
            })
            .collect(),
    }
}

/// (scene id, scene frame, start frame of the scene's first clip) at a global frame.
fn pixel<'a, 'm: 'a>(previewer: &mut Previewer<'a, 'm, ThreeScenes>, frame: usize) -> (u8, u8, u8) {
    let mut renderer = CpuFrameRenderer::default();
    let rgba = previewer.render(frame, &mut renderer).unwrap().pixels;
    (rgba[0] / 50, rgba[1], rgba[2])
}

fn split_doc() -> TimingDoc {
    let c = c_key();
    doc(&[(&c, 0, 0), ("a", 0, 6), ("b", 3, 0), ("a", 4, 0)])
}

#[test]
fn without_timing_the_code_layout_is_unchanged() {
    let video = ThreeScenes::new();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    let frames: Vec<_> = previewer
        .timeline()
        .scenes
        .iter()
        .map(|s| s.frames.clone())
        .collect();
    assert_eq!(frames, [0..10, 8..20, 20..30]);
    assert_eq!(previewer.timeline().duration_in_frames, 30);
    assert_eq!(
        pixel(&mut previewer, 9),
        (2, 1, 8),
        "b overlaps a and draws last"
    );
    assert_eq!(pixel(&mut previewer, 25), (3, 5, 20));
}

#[test]
fn reorder_trim_and_split_change_preview_frames_without_rebuilding() {
    let video = ThreeScenes::new();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    let report = previewer.set_timing(Some(&split_doc())).unwrap();
    assert!(report.orphan_clips.is_empty() && report.appended_scenes.is_empty());

    let timeline = previewer.timeline();
    let layout: Vec<_> = timeline
        .scenes
        .iter()
        .map(|s| {
            (
                s.editor_instance_key.clone(),
                s.frames.clone(),
                s.trim_start,
                s.trim_end,
            )
        })
        .collect();
    assert_eq!(
        layout,
        [
            (None, 0..10, 0, 0),
            (Some("a".to_owned()), 10..14, 0, 6),
            // A trimmed start is a hard cut: b's overlap with its predecessor is dropped.
            (Some("b".to_owned()), 14..21, 3, 0),
            (Some("a#1".to_owned()), 21..27, 4, 0),
        ]
    );
    assert_eq!(timeline.duration_in_frames, 27);

    // Scenes see their own frame numbers; get_scene_info answers with the first clip.
    assert_eq!(pixel(&mut previewer, 0), (3, 0, 0));
    assert_eq!(pixel(&mut previewer, 13), (1, 3, 10));
    assert_eq!(pixel(&mut previewer, 14), (2, 3, 14));
    assert_eq!(pixel(&mut previewer, 20), (2, 9, 14));
    assert_eq!(pixel(&mut previewer, 21), (1, 4, 10));
    assert_eq!(pixel(&mut previewer, 26), (1, 9, 10));

    // Back to the code order.
    previewer.set_timing(None).unwrap();
    assert_eq!(previewer.timeline().duration_in_frames, 30);
    assert_eq!(pixel(&mut previewer, 9), (2, 1, 8));
}

#[test]
fn moving_an_overlapping_scene_first_clamps_its_overlap() {
    let video = ThreeScenes::new();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    previewer
        .set_timing(Some(&doc(&[("b", 0, 0), ("a", 0, 0)])))
        .unwrap();
    let frames: Vec<_> = previewer
        .timeline()
        .scenes
        .iter()
        .map(|s| s.frames.clone())
        .collect();
    // c is not in the document and is appended.
    assert_eq!(frames, [0..10, 10..20, 20..30]);
    assert_eq!(pixel(&mut previewer, 0), (2, 0, 0));
}

#[test]
fn rejected_documents_keep_the_previous_timing() {
    let video = ThreeScenes::new();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    previewer.set_timing(Some(&split_doc())).unwrap();
    let too_long = doc(&[("a", 5, 5)]);
    assert!(previewer.set_timing(Some(&too_long)).is_err());
    let bad_version = TimingDoc {
        version: 9,
        clips: vec![],
    };
    assert!(previewer.set_timing(Some(&bad_version)).is_err());
    assert_eq!(previewer.timeline().duration_in_frames, 27);

    let report = previewer
        .set_timing(Some(&doc(&[("gone", 0, 0), ("a", 0, 0)])))
        .unwrap();
    assert_eq!(report.orphan_clips, ["gone"]);
    assert_eq!(report.appended_scenes, ["b".to_owned(), c_key()]);
}

#[test]
fn render_path_matches_preview_frames() {
    let video = ThreeScenes::new();
    let timing = split_doc();
    let options = RenderOptions {
        timing: Some(timing.clone()),
        ..Default::default()
    };
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    previewer.set_timing(Some(&timing)).unwrap();
    let mut renderer = CpuFrameRenderer::default();
    for frame in [3, 13, 24] {
        let rendered = crate::render_frame(
            frame,
            &video,
            crate::cpu::CpuRenderingBackend::default(),
            &options,
        )
        .unwrap();
        let previewed = previewer.render(frame, &mut renderer).unwrap();
        assert_eq!(rendered, previewed.pixels, "frame {frame}");
    }
}

#[test]
fn scene_audio_follows_its_clip_and_is_trimmed_with_it() {
    let video = ThreeScenes::new();
    let c = c_key();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    let track = |previewer: &Previewer<'_, '_, ThreeScenes>| {
        let timeline = previewer.resolved_timeline();
        let tb = previewer.runtime_time_base();
        let tracks = &timeline.audio_map.as_ref().unwrap().0;
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        (
            t.range.start.to_seconds(&tb),
            t.range.end.to_seconds(&tb),
            f64::from(t.mix.offset),
        )
    };
    let close = |a: (f64, f64, f64), b: (f64, f64, f64)| {
        assert!(
            (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3 && (a.2 - b.2).abs() < 1e-3,
            "{a:?} != {b:?}"
        );
    };
    close(track(&previewer), (2.0, 3.0, 0.0));

    // c moves first and loses 0.2 s at the start and 0.3 s at the end.
    previewer
        .set_timing(Some(&doc(&[(&c, 2, 3), ("a", 0, 0)])))
        .unwrap();
    close(track(&previewer), (0.0, 0.5, 0.2));

    // Split in two: each clip plays its own part of the file.
    previewer
        .set_timing(Some(&doc(&[(&c, 0, 6), ("a", 0, 0), (&c, 4, 0)])))
        .unwrap();
    let timeline = previewer.resolved_timeline();
    let tb = previewer.runtime_time_base();
    let parts: Vec<_> = timeline
        .audio_map
        .as_ref()
        .unwrap()
        .0
        .iter()
        .map(|t| {
            (
                (t.range.start.to_seconds(&tb) * 10.).round() as i64,
                (t.range.end.to_seconds(&tb) * 10.).round() as i64,
                (f64::from(t.mix.offset) * 10.).round() as i64,
            )
        })
        .collect();
    // c#0 at 0.0..0.4 plays 0.0..0.4; c#1 at 1.4..2.0 plays 0.4..1.0
    assert_eq!(parts, [(0, 4, 0), (14, 20, 4)]);
}

/// Random documents never panic: each is either rejected with a reason, leaving the
/// previous layout, or laid out as clips that tile the timeline in document order.
#[test]
fn random_documents_are_laid_out_or_rejected_whole() {
    use rand::{Rng, SeedableRng, rngs::StdRng};

    let video = ThreeScenes::new();
    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    let keys = [
        "a".to_owned(),
        "b".to_owned(),
        c_key(),
        "gone".to_owned(),
        String::new(),
    ];
    let trims = [0, 1, 4, 9, 10, 11, usize::MAX / 2, usize::MAX];
    let mut rng = StdRng::seed_from_u64(0x7111_6000);
    let (mut applied, mut rejected) = (0, 0);

    for _ in 0..2000 {
        let before = previewer.timeline_report();
        let doc = TimingDoc {
            version: if rng.random_ratio(1, 50) { 2 } else { 1 },
            clips: (0..rng.random_range(0..12))
                .map(|_| TimingClip {
                    key: keys[rng.random_range(0..keys.len())].clone(),
                    trim_start_frames: trims[rng.random_range(0..trims.len())],
                    trim_end_frames: trims[rng.random_range(0..trims.len())],
                })
                .collect(),
        };
        if let Err(error) = previewer.set_timing(Some(&doc)) {
            rejected += 1;
            assert_ne!(error.to_string(), "");
            let after = previewer.timeline_report();
            assert_eq!(after.duration_frames, before.duration_frames);
            assert_eq!(after.scenes.len(), before.scenes.len());
        } else {
            applied += 1;
            let report = previewer.timeline_report();
            let mut end = 0;
            for scene in &report.scenes {
                // Only `b` overlaps its previous neighbour, by 2 frames, and only
                // on an untrimmed start edge.
                let overlap = end - scene.start_frame;
                assert!(overlap == 0 || (overlap <= 2 && scene.trim_start_frames == 0));
                assert!(scene.trim_start_frames + scene.trim_end_frames < 10);
                assert_eq!(
                    scene.end_frame - scene.start_frame,
                    10 - scene.trim_start_frames - scene.trim_end_frames + overlap
                );
                end = scene.end_frame;
            }
            assert_eq!(report.duration_frames, end);
            for frame in [0, end / 2, end.saturating_sub(1)] {
                pixel(&mut previewer, frame);
            }
        }
    }
    assert!(
        applied > 100 && rejected > 100,
        "{applied} applied, {rejected} rejected"
    );

    let alphabet = b"{}[]\":,0123456789-versionclipskeytrim_start_framestrim_end_frames";
    for _ in 0..2000 {
        let length = rng.random_range(0..64);
        let bytes: Vec<u8> = (0..length)
            .map(|_| alphabet[rng.random_range(0..alphabet.len())])
            .collect();
        let _ = TimingDoc::from_json(&String::from_utf8_lossy(&bytes));
    }
    let huge = format!(
        r#"{{"version":1,"clips":[],"pad":"{}"}}"#,
        "x".repeat(crate::MAX_TIMING_DOC_BYTES)
    );
    assert!(matches!(
        TimingDoc::from_json(&huge),
        Err(crate::TimingError::TooLarge(_))
    ));
}

/// Long enough that every render below is encoded in more than one segment (a segment
/// is at least two GOPs of 12 frames).
const SQUARE_FRAMES: usize = 40;

/// A white square that moves 4 px per scene frame (wrapping at 48 px) over a background
/// per scene, so a wrong order, trim or scene frame changes the picture.
#[derive(Debug)]
struct Square(&'static str, Color);

impl Scene for Square {
    fn duration(&self) -> crate::Duration<'_> {
        crate::Duration::Frames(SQUARE_FRAMES)
    }

    fn editor_instance_key(&self) -> Option<&str> {
        Some(self.0)
    }

    fn render_frame<'a>(&'a self, frame: Frame, _: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let x = (frame.index * 4) % 48;
        crate::svgr!(
            <g>
                <rect x="0" y="0" width="64" height="48" fill={self.1} />
                <rect x={x} y="16" width="16" height="16" fill="#fff" />
            </g>
        )
    }
}

struct Squares([Square; 3]);

impl Video for Squares {
    const FPS: usize = 10;
    const WIDTH: usize = 64;
    const HEIGHT: usize = 48;

    fn duration(&self) -> crate::Duration<'_> {
        crate::Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn define_scenes(&self) -> crate::Scenes<'_> {
        let scenes: Vec<&dyn Scene> = self.0.iter().map(|scene| scene as _).collect();
        crate::Scenes::from(scenes)
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        crate::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 48" width="64" height="48">
                {ctx.render_scenes(&frame)}
            </svg>
        )
    }
}

/// Whether two pixels are the same within odiff's YIQ colour distance `threshold`.
fn yiq_close(expected: &[u8], actual: &[u8], threshold: f64) -> bool {
    let yiq = |p: &[u8]| {
        let (r, g, b) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        (
            r * 0.298_895_31 + g * 0.586_622_47 + b * 0.114_482_23,
            r * 0.595_977_99 - g * 0.274_176_10 - b * 0.321_801_89,
            r * 0.211_470_17 - g * 0.522_617_11 + b * 0.311_146_94,
        )
    };
    let (e, a) = (yiq(expected), yiq(actual));
    let delta =
        0.5053 * (e.0 - a.0).powi(2) + 0.299 * (e.1 - a.1).powi(2) + 0.1957 * (e.2 - a.2).powi(2);
    delta <= 35215. * threshold * threshold
}

/// RGBA of a frame of an encoded file.
fn decoded_frame(path: &std::path::Path, fps: usize, frame: usize) -> Vec<u8> {
    let mut decoder = unsafe { crate::media::FFmpegDecoder::new(path, fps, 1) }.unwrap();
    assert!(unsafe { decoder.decode_up_to(frame as i64) }.unwrap());
    let image = unsafe {
        decoder
            .get_raw_frame()
            .convert_last_decoded_frame_into_svg_image(None)
    }
    .unwrap();
    assert_eq!((image.width, image.height), (64, 48));
    image.data.to_vec()
}

fn squares() -> Squares {
    Squares([
        Square("red", Color::rgb(200, 30, 30)),
        Square("green", Color::rgb(30, 160, 60)),
        Square("blue", Color::rgb(30, 60, 200)),
    ])
}

/// Short `WebM` segments carry no packet durations; joined segments must still continue
/// one frame after each other instead of overlapping on the last timestamp.
#[test]
fn webm_segments_join_without_losing_frames() {
    if !crate::media::has_encoder("libvpx-vp9") {
        eprintln!("skipping: this FFmpeg build has no libvpx-vp9 encoder");
        return;
    }
    let video = squares();
    let path = std::env::temp_dir().join(format!("fframes-webm-{}.webm", uuid::Uuid::new_v4()));
    // Twelve segments of 10 frames, shorter than libvpx's lookahead, so the encoder
    // flushes each one at its end without packet durations.
    let backend = crate::cpu::CpuRenderingBackend {
        concurrency: 12,
        ..Default::default()
    };
    crate::render(&path, &video, backend, &RenderOptions::default()).unwrap();
    let decoder = unsafe { crate::media::FFmpegDecoder::new(&path, Squares::FPS, 1) }.unwrap();
    assert_eq!(decoder.get_last_frame_offset(), Some(119));
    drop(decoder);

    let mut previewer = Previewer::new(&video, &RenderOptions::default()).unwrap();
    let mut renderer = CpuFrameRenderer::default();
    for frame in [10, 70, 110, 119] {
        let expected = previewer.render(frame, &mut renderer).unwrap().pixels;
        let decoded = decoded_frame(&path, Squares::FPS, frame);
        let different = expected
            .chunks(4)
            .zip(decoded.chunks(4))
            .filter(|(e, a)| !yiq_close(e, a, 0.4))
            .count();
        assert_eq!(different, 0, "frame {frame} differs from the preview");
    }
    let _ = std::fs::remove_file(&path);
}
