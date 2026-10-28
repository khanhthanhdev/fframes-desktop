//! A presenter-led physics, mathematics and Rust lesson built with fframes-kit.
mod diagrams;

use std::sync::Arc;

use fframes::{AudioMap, Color, Duration, FFramesContext, Frame, Scenes, Svgr, Video};
use fframes_kit::{
    slides::{Deck, Slide, SlideLayout},
    theme::{Colors, Theme},
};
use fframes_studio_kit::PropsStore;

/// Launch speed in metres per second. The example assumes no air resistance.
pub const SPEED: f32 = 20.0;
/// Uniform gravitational acceleration, in metres per second squared.
pub const GRAVITY: f32 = 9.81;

/// Position in metres after `t` seconds, from ground level, with `angle` in radians.
pub fn position(t: f32, angle: f32) -> (f32, f32) {
    (
        SPEED * angle.cos() * t,
        SPEED * angle.sin() * t - 0.5 * GRAVITY * t * t,
    )
}

/// Flight time until the projectile returns to its launch height.
pub fn flight_time(angle: f32) -> f32 {
    2.0 * SPEED * angle.sin() / GRAVITY
}

fn slides() -> Vec<Slide<'static>> {
    vec![
        Slide {
            build: Some(4.8),
            hold: 1.6,
            notes: Some(
                "Ask the audience to imagine throwing a ball. Why does its path curve? We will connect one picture to two equations and then a short Rust function.",
            ),
            ..Slide::new("launch", SlideLayout::Custom(diagrams::launch))
        },
        Slide {
            hold: 3.0,
            notes: Some(
                "This is the ideal projectile model. Gravity is uniform, drag is ignored, and the ball lands at its original height. The 45-degree result later depends on these assumptions.",
            ),
            ..Slide::new(
                "assumptions",
                SlideLayout::Bullets {
                    title: "Start with a simple model",
                    items: vec![
                        "No air resistance: gravity is the only force",
                        "Constant gravity: g = 9.81 m/s² downward",
                        "Launch and landing are at the same height",
                    ],
                    stagger: 1.4,
                },
            )
        },
        Slide {
            build: Some(5.8),
            hold: 2.2,
            notes: Some(
                "Resolve the launch speed into horizontal and vertical components. The horizontal velocity remains constant. The vertical velocity decreases at 9.81 metres per second each second. At the apex only the vertical velocity is zero.",
            ),
            ..Slide::new("components", SlideLayout::Custom(diagrams::components))
        },
        Slide {
            build: Some(4.0),
            hold: 3.0,
            notes: Some(
                "Set y(T) = 0 to get the nonzero flight time T = 2 v0 sin(theta) / g. Substitute into x(T): R = v0² sin(2 theta) / g. Sine is maximised at 2 theta = 90 degrees, so theta = 45 degrees. Complementary angles have equal range but different peak heights. This changes with drag or unequal launch and landing heights.",
            ),
            ..Slide::new("range", SlideLayout::Custom(diagrams::range))
        },
        Slide {
            hold: 6.0,
            notes: Some(
                "The same analytic equations drive the rendered ball. Here vx and vy are the launch velocity components, in metres per second, and t is in seconds. A screen has downward-positive y, so drawing converts with screen_y = ground_y - scale * y. Stop at the flight time to avoid drawing underground.",
            ),
            ..Slide::new(
                "code",
                SlideLayout::Code {
                    title: "The picture is just a function of time",
                    language: "Rust · metres / seconds",
                    code: "fn position(t: f32, vx: f32, vy: f32) -> (f32, f32) {\n    let g = 9.81;\n    let x = vx * t;\n    let y = vy * t - 0.5 * g * t * t;\n    (x, y)\n}\n\n// Draw at (origin_x + scale * x, ground_y - scale * y)",
                },
            )
        },
        Slide {
            hold: 3.0,
            notes: Some(
                "Ask: do 30 and 60 degrees have the same flight time? No. They have the same ideal range, but the 60-degree throw travels higher and stays in the air longer. Then ask which assumptions would change the 45-degree answer.",
            ),
            ..Slide::new(
                "recap",
                SlideLayout::Bullets {
                    title: "One throw. Three things to remember.",
                    items: vec![
                        "Horizontal and vertical motion share one clock",
                        "A straight x(t) plus a quadratic y(t) makes a parabola",
                        "45° maximises range only in the ideal model",
                    ],
                    stagger: 1.4,
                },
            )
        },
    ]
}

/// Six slides, including editable kit layouts and custom animated vector diagrams.
#[derive(Debug)]
pub struct ProjectileMotionVideo {
    deck: Deck<'static>,
}

impl ProjectileMotionVideo {
    /// Connects the kit's editable text and suggested stops to the Studio props store.
    pub fn new(props: Arc<PropsStore>) -> Self {
        let mut theme = Theme {
            colors: Colors {
                background: Color::hex("#101c28"),
                surface: Color::hex("#192c3c"),
                text: Color::hex("#e7edf2"),
                title: Color::hex("#fff4df"),
                muted: Color::hex("#93aabd"),
                accent: Color::hex("#ffbf69"),
                highlight: Color::hex("#60d5d0"),
                caption: Color::hex("#c3d3df"),
            },
            ..Theme::default()
        };
        theme.spacing.margin = 144.0;
        theme.typography.heading.size = 72;
        theme.typography.body.size = 44;
        Self {
            deck: Deck::new(theme, slides()).with_props(props, PropsStore::slide_view),
        }
    }
}

impl Video for ProjectileMotionVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;

    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        // Deliberately silent: the presenter supplies the narration.
        AudioMap::none()
    }

    fn define_scenes(&self) -> Scenes<'_> {
        self.deck.scenes()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width="1920" height="1080">
                {ctx.render_scenes(&frame)}
            </svg>
        )
    }
}
