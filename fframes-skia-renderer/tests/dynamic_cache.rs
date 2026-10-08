//! Cached resources must produce the same full frame as a fresh renderer when
//! geometry, paint state, and the surrounding compositing state change.

use std::fmt::Write;

use fframes::{Color, FrameRenderer, usvgr};
use fframes_skia_renderer::{SkiaBackend, SkiaCacheConfig, SkiaCpuCtx, SkiaFrameRenderer};

const SIZE: usize = 100;

fn tree(content: &str) -> usvgr::Tree {
    usvgr::Tree::from_str(
        &format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{content}</svg>"#
        ),
        &usvgr::Options::default(),
        &usvgr::fontdb::Database::new(),
    )
    .expect("test SVG")
}

fn frames() -> Vec<usvgr::Tree> {
    let mut frames = Vec::new();
    for i in 0..8 {
        // Repeat geometry across changing gradients, strokes, fractional
        // transforms, clip paths, masks, group opacity, and paint order.
        frames.push(tree(&format!(
            r##"<defs>
                <linearGradient id="g" x2="100%" gradientTransform="rotate({i} .5 .5)" spreadMethod="reflect">
                    <stop offset="0" stop-color="#{:06x}" stop-opacity=".7"/>
                    <stop offset="1" stop-color="#aaff33"/>
                </linearGradient>
                <radialGradient id="r" cx=".4" cy=".6" r=".8"><stop stop-color="blue"/><stop offset="1" stop-color="red"/></radialGradient>
                <clipPath id="c"><circle cx="50" cy="50" r="{}"/></clipPath>
                <mask id="m"><rect width="100" height="100" fill="white"/><circle cx="60" cy="50" r="{}" fill="black"/></mask>
                <filter id="f"><feGaussianBlur stdDeviation=".8"/></filter>
            </defs>
            <g transform="translate({} .25) rotate({i} 50 50)" opacity="{}" clip-path="url(#c)" mask="url(#m)">
                <path d="M10 10 Q50 80 90 10 L90 90 C60 60 40 100 10 90 Z M30 30 L70 30 L70 70 L30 70 Z"
                    fill="url(#g)" fill-rule="{}" stroke="url(#r)" stroke-width="{}"
                    stroke-dasharray="{} 3" stroke-dashoffset="{i}" stroke-linecap="{}" stroke-linejoin="{}"
                    shape-rendering="{}" paint-order="{}"/>
                <circle cx="50" cy="50" r="20" fill="#5544ff" opacity=".4"/>
            </g>
            <g filter="url(#f)" transform="translate({i} 0)"><path d="M20 20 L70 30 L40 70 Z" fill="#88ccff"/></g>"##,
            0x00ff_0000 + i * 31,
            42 + i,
            4 + i,
            i as f32 / 4.0,
            0.3 + i as f32 / 12.0,
            if i % 2 == 0 { "evenodd" } else { "nonzero" },
            1 + i,
            2 + i,
            if i % 2 == 0 { "round" } else { "square" },
            if i % 2 == 0 { "bevel" } else { "miter" },
            if i % 2 == 0 { "crispEdges" } else { "geometricPrecision" },
            if i % 2 == 0 { "stroke fill" } else { "fill stroke" },
        )));
    }

    // Same point count and bounds, but an interior point beyond a strided
    // fingerprint changes. Dynamic geometry must examine every point.
    for offset in [0, 12, 0] {
        let mut d = "M0 0 L100 0 L100 100 L0 100 M0 50".to_owned();
        for i in 1..300 {
            let y = if i == 137 { 50 + offset } else { 50 };
            write!(d, " L{} {y}", i as f32 / 3.0).unwrap();
        }
        frames.push(tree(&format!(
            r#"<path d="{d}" fill="none" stroke="red" stroke-width="2"/>"#
        )));
    }
    // Identical points can describe different curves; verbs are part of identity.
    for d in [
        "M10 10 L50 90 L90 10",
        "M10 10 Q50 90 90 10",
        "M10 10 L50 90 L90 10",
    ] {
        frames.push(tree(&format!(
            r#"<path d="{d}" fill="none" stroke="blue" stroke-width="3"/>"#
        )));
    }
    frames
}

fn compare_frames(backend: &impl SkiaBackend, config: SkiaCacheConfig) {
    let mut cached = SkiaFrameRenderer::new(backend).with_cache_config(config);
    let mut previous = None;
    let mut changed = false;
    let frames = frames();
    for (i, tree) in frames
        .iter()
        .chain(frames.iter().rev())
        .chain(&frames)
        .enumerate()
    {
        let actual = cached
            .render_tree(tree, Color::TRANSPARENT, SIZE as u32, SIZE as u32)
            .expect("cached frame");
        let expected = SkiaFrameRenderer::new(backend)
            .render_tree(tree, Color::TRANSPARENT, SIZE as u32, SIZE as u32)
            .expect("fresh frame");
        assert_eq!(actual.pixels, expected.pixels, "frame {i}");
        changed |= previous.as_ref().is_some_and(|p| *p != actual.pixels);
        previous = Some(actual.pixels);
    }
    assert!(changed, "the frames must exercise changing output");
}

#[test]
fn dynamic_resources_preserve_raster_output() {
    let backend = SkiaCpuCtx::new(SIZE, SIZE);
    for config in configurations() {
        compare_frames(&backend, config);
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
#[test]
fn dynamic_resources_preserve_metal_output() {
    if fframes_skia_renderer::metal::metal_rs::Device::system_default().is_none() {
        eprintln!("Metal device unavailable; skipping GPU comparison");
        return;
    }
    let backend =
        fframes_skia_renderer::metal::SkiaMetalCtx::new(SIZE, SIZE).expect("Metal context");
    for config in configurations() {
        compare_frames(&backend, config);
    }
}

fn configurations() -> [SkiaCacheConfig; 3] {
    [
        SkiaCacheConfig::default(),
        SkiaCacheConfig {
            text_capacity: 0,
            geometry_capacity: 0,
            geometry_bytes: 0,
        },
        SkiaCacheConfig {
            text_capacity: 100_000,
            geometry_capacity: 100_000,
            geometry_bytes: 100_000 * 512,
        },
    ]
}
