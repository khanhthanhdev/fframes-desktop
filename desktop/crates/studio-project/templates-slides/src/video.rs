use std::sync::Arc;

use fframes::{
    AudioMap, Color, Duration, FFramesContext, Frame, Scenes, Styles, StylesError, Svgr, Video,
};
use fframes_kit::slides::{Deck, Slide, SlideLayout};
use fframes_kit::theme::{Colors, Theme};
use fframes_studio_kit::PropsStore;

/// The slides in play order. Each slide is one scene and one clip in Studio's timeline,
/// found by its key: keep a key when you change its slide. Speaker notes and the stops
/// after each bullet live in slides.json.
fn slides() -> Vec<Slide<'static>> {
    vec![
        Slide::new(
            "intro",
            SlideLayout::Title {
                title: "Your deck starts here",
                subtitle: Some("Slides that are also a video"),
            },
        ),
        Slide::new(
            "agenda",
            SlideLayout::bullets(
                "What a slide does",
                vec![
                    "Every slide is a scene of the video",
                    "Bullets come in one at a time",
                    "The deck stops where slides.json says",
                ],
            ),
        ),
        Slide::new(
            "present",
            SlideLayout::Section {
                title: "Present it, or export an HTML deck",
            },
        ),
        Slide::new(
            "outputs",
            SlideLayout::TwoColumns {
                title: "One source, two outputs",
                left: "Video\nRender an MP4 with the fframes command line",
                right: "Deck\nPresent in Studio or share an HTML deck",
            },
        ),
        Slide::new(
            "edit",
            SlideLayout::bullets(
                "Make it yours",
                vec![
                    "Change slide text in the Props panel",
                    "Add a slide in src/video.rs",
                    "Write speaker notes in slides.json",
                ],
            ),
        ),
        Slide::new(
            "thanks",
            SlideLayout::Quote {
                text: "Thank you",
                author: Some("Your name"),
            },
        ),
    ]
}

/// The deck as a video. The Props panel edits slide text through `props`.
#[derive(Debug)]
pub struct StudioVideo {
    deck: Deck<'static>,
}

impl StudioVideo {
    /// Resolves the design tokens into the deck's theme once, so `render_frame` only reads.
    pub fn new(props: Arc<PropsStore>) -> Result<Self, StylesError> {
        let styles = Styles::from_json_str(include_str!("../style/tokens.json"))?;
        let text = styles.color("color.text")?;
        let theme = Theme {
            colors: Colors {
                background: styles.color("color.background")?,
                text,
                title: text,
                ..Colors::default()
            },
            ..Theme::default()
        };
        let deck = Deck::new(theme, slides()).with_props(props, PropsStore::slide_view);
        Ok(Self { deck })
    }

    /// The slides in play order, with their keys and reveal times.
    pub fn deck(&self) -> &Deck<'static> {
        &self.deck
    }
}

impl Video for StudioVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn editor_instance_key(&self) -> Option<&str> {
        Some("slides-video")
    }

    /// The sum of the slides, in the order Studio's timeline.json gives them.
    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    /// Lists each slide once. Studio's timeline orders, trims and splits them.
    fn define_scenes(&self) -> Scenes<'_> {
        self.deck.scenes()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width={Self::WIDTH} height={Self::HEIGHT}>
                {ctx.render_scenes(&frame)}
            </svg>
        )
    }
}
