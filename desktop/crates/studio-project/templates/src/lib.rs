use fframes::{
    Color, Duration, EditorObjectKey, Frame, Scene, Styles, StylesError, Svgr, Typography, Video,
};

// How the scenes make the video: size, frame rate and the scene list.
mod video;

/// Design tokens every scene draws with, resolved once in [`StudioVideo::new`] and
/// scaled from the tokens' 1920x1080 design canvas to the video's frame.
#[derive(Debug, Clone)]
struct Look {
    background: Color,
    text: Color,
    title: Typography,
    margin: f32,
    /// The video's frame, `Video::WIDTH` by `Video::HEIGHT`.
    width: f32,
    height: f32,
}

/// A title card. Studio's timeline finds it by `key` (its `editor_instance_key`), so keep
/// the key stable when you change the scene.
#[derive(Debug)]
pub struct TitleScene {
    key: &'static str,
    headline: &'static str,
    seconds: f32,
    look: Look,
    title_id: String,
}

impl TitleScene {
    fn new(
        key: &'static str,
        headline: &'static str,
        seconds: f32,
        look: &Look,
    ) -> Result<Self, StylesError> {
        let title_key = EditorObjectKey::new(key, "starter-title", "headline", "primary")
            .and_then(|key| {
                key.with_source_anchor(
                    "src/lib.rs",
                    "render_frame",
                    Some(concat!("studio-title-source-", "anchor")),
                )
            })
            .and_then(|key| {
                key.with_style_tokens([
                    "color.background",
                    "color.text",
                    "typography.title",
                    "spacing.margin",
                ])
            })
            .map_err(|error| StylesError::Invalid {
                token: "editor.identity".into(),
                reason: error.to_string(),
            })?;
        Ok(Self {
            key,
            headline,
            seconds,
            look: look.clone(),
            title_id: title_key.render_id(),
        })
    }
}

impl Scene for TitleScene {
    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(self.seconds)
    }

    fn name(&self) -> &'static str {
        self.key
    }

    fn editor_instance_key(&self) -> Option<&str> {
        Some(self.key)
    }

    fn render_frame<'a>(
        &'a self,
        _frame: Frame,
        _ctx: &fframes::FFramesContext<'a, '_>,
    ) -> Svgr<'a> {
        let Look {
            background,
            text,
            title,
            margin,
            width,
            height,
        } = &self.look;
        // studio-title-source-anchor
        fframes::svgr!(
            <g>
                <rect width={*width} height={*height} fill={*background} />
                <g id={self.title_id.as_str()}>
                    <text x={*margin} y={height * 0.28} font-family={title.family.as_str()} font-size={title.size} font-weight={title.weight} fill={*text}>
                        {self.headline}
                    </text>
                </g>
            </g>
        )
    }
}

pub struct StudioVideo {
    scenes: [TitleScene; 3],
}

impl StudioVideo {
    /// Parses the embedded design tokens and resolves every token the video uses, once.
    /// `render_frame` only reads the resolved values, so it never fails or parses.
    pub fn new() -> Result<Self, StylesError> {
        let styles = Styles::from_json_str(include_str!("../style/tokens.json"))?;
        let (width, height) = (Self::WIDTH as f32, Self::HEIGHT as f32);
        // Sizes in the tokens are for 1920x1080; fit them to a frame of another size or shape.
        let scale = (width / 1920.0).min(height / 1080.0);
        let mut title = styles.typography("typography.title")?.clone();
        title.size *= scale;
        let look = Look {
            background: styles.color("color.background")?,
            text: styles.color("color.text")?,
            title,
            margin: styles.dimension("spacing.margin")? * scale,
            width,
            height,
        };
        Ok(Self {
            scenes: [
                TitleScene::new("intro", "Your video starts here", 3.0, &look)?,
                TitleScene::new("story", "Every scene is a clip", 4.0, &look)?,
                TitleScene::new("outro", "Trim it in Studio", 3.0, &look)?,
            ],
        })
    }
}
