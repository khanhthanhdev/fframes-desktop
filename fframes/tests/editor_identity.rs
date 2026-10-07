use fframes::{
    AudioMap, Color, CpuFrameRenderer, Duration, EditorGeometrySupport, EditorObjectKey,
    FFramesContext, Frame, Previewer, RenderOptions, Svgr, Video,
};

fn key(object: &str, repeat: &str) -> EditorObjectKey {
    EditorObjectKey::new("intro", "title-card", object, repeat).expect("valid editor key")
}

#[test]
fn explicit_source_anchor_survives_svg_identity_encoding() {
    let anchored = key("headline", "primary")
        .with_source_anchor("src/lib.rs", "render_frame", Some("unique-source-marker"))
        .expect("valid source anchor")
        .with_style_tokens(["color.text", "typography.title"])
        .expect("valid token bindings");
    let decoded = EditorObjectKey::from_render_id(&anchored.render_id()).unwrap();
    assert_eq!(decoded, anchored);
    assert_eq!(decoded.source_anchor, anchored.source_anchor);
    assert_eq!(decoded.style_tokens, anchored.style_tokens);

    let token_only = key("headline", "primary")
        .with_style_tokens(["color.text", "typography.title"])
        .expect("style bindings do not require a source anchor");
    let decoded = EditorObjectKey::from_render_id(&token_only.render_id()).unwrap();
    assert_eq!(decoded, token_only);
    assert_eq!(decoded.source_anchor, None);
    assert_eq!(decoded.style_tokens, token_only.style_tokens);

    assert!(
        key("headline", "primary")
            .with_source_anchor("../outside.rs", "render_frame", None)
            .is_err()
    );
}

struct EditorFixture;

impl Video for EditorFixture {
    const FPS: usize = 30;
    const WIDTH: usize = 320;
    const HEIGHT: usize = 180;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Frames(1)
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn render_frame<'a>(&'a self, _: Frame, _: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let primary = key("heading", "primary")
            .with_style_tokens(["color.text"])
            .expect("explicit style binding")
            .render_id();
        let secondary = key("heading", "secondary").render_id();
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width="160" height="90" viewBox="0 0 160 90">
                <g id={primary} transform="translate(20 10)">
                    <rect x="0" y="0" width="30" height="10" fill="#ff0000" />
                </g>
                <g id={secondary} transform="translate(60 20)">
                    <rect x="0" y="0" width="20" height="10" fill="#00ff00" />
                </g>
            </svg>
        )
        .with_editor_object(&key("canvas", "root"))
    }
}

#[test]
fn annotated_groups_keep_stable_keys_transforms_order_and_video_pixel_bounds() {
    let mut previewer =
        Previewer::new(&EditorFixture, &RenderOptions::default()).expect("fixture previewer");
    let mut renderer = CpuFrameRenderer::default();
    let (frame, geometry) = previewer
        .render_with_editor_geometry(0, &mut renderer)
        .expect("render annotated frame");
    let geometry = geometry.expect("valid editor geometry");

    assert_eq!((frame.width, frame.height), (320, 180));
    assert_eq!(geometry.video_width, 320);
    assert_eq!(geometry.video_height, 180);
    assert_eq!(geometry.objects.len(), 3);
    let primary = geometry
        .objects
        .iter()
        .find(|object| object.key == key("heading", "primary"))
        .expect("primary object converted");
    let secondary = geometry
        .objects
        .iter()
        .find(|object| object.key == key("heading", "secondary"))
        .expect("repeated object converted");
    assert!(primary.paint_order < secondary.paint_order);
    assert_eq!(primary.support, EditorGeometrySupport::ExactBounds);
    assert_eq!(primary.style_tokens, ["color.text"]);
    assert_eq!(primary.source_anchor, None);
    assert_eq!(primary.parent, Some(key("canvas", "root")));
    assert!((primary.bounds.x - 40.0).abs() < 0.01);
    assert!((primary.bounds.y - 20.0).abs() < 0.01);
    assert!((primary.bounds.width - 60.0).abs() < 0.01);
    assert!((primary.bounds.height - 20.0).abs() < 0.01);
    let red = ((25 * frame.width as usize) + 50) * 4;
    assert_eq!(&frame.pixels[red..red + 4], &[255, 0, 0, 255]);

    previewer.set_scale(0.5);
    let (thumbnail, thumbnail_geometry) = previewer
        .render_with_editor_geometry(0, &mut renderer)
        .expect("render scaled frame");
    assert_eq!((thumbnail.width, thumbnail.height), (160, 90));
    assert_eq!(
        thumbnail_geometry.expect("scaled geometry is available"),
        geometry
    );
}

#[test]
fn semantic_keys_are_reversible_and_repeated_instances_do_not_alias() {
    let original = EditorObjectKey::new("sc:ene", "компонент", "title.card", "copy/2")
        .expect("valid Unicode semantic keys");
    assert_eq!(
        EditorObjectKey::from_render_id(&original.render_id()),
        Some(original)
    );
    assert!(EditorObjectKey::from_render_id("fframes.editor.v1.0g.aa.bb.cc").is_none());
    assert!(EditorObjectKey::new("scene", "component", "", "primary").is_err());
    assert_ne!(key("heading", "primary"), key("heading", "secondary"));
}

#[cfg(not(feature = "compile-time-svgtree"))]
#[test]
fn runtime_string_scene_fragments_keep_the_editor_root_identity() {
    struct FragmentFixture;

    impl Video for FragmentFixture {
        const FPS: usize = 30;
        const WIDTH: usize = 320;
        const HEIGHT: usize = 180;
        const BACKGROUND_COLOR: Color = Color::BLACK;

        fn duration(&self) -> Duration<'_> {
            Duration::Frames(1)
        }

        fn audio(&self) -> AudioMap<'_> {
            AudioMap::none()
        }

        fn render_frame<'a>(&'a self, _: Frame, _: &FFramesContext<'a, '_>) -> Svgr<'a> {
            Svgr::from(r##"<rect x="5" y="7" width="10" height="8" fill="#ff0000"/>"##.to_owned())
                .with_editor_object(&key("fragment", "primary"))
        }
    }

    let mut previewer =
        Previewer::new(&FragmentFixture, &RenderOptions::default()).expect("fixture previewer");
    let mut renderer = CpuFrameRenderer::default();
    let (_, geometry) = previewer
        .render_with_editor_geometry(0, &mut renderer)
        .expect("render runtime SVG fragment");
    let geometry = geometry.expect("fragment identity is present");
    assert!(
        geometry
            .objects
            .iter()
            .any(|object| object.key == key("fragment", "primary"))
    );
}
