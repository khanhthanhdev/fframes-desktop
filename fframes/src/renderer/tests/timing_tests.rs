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
        timing: Some(&timing),
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
