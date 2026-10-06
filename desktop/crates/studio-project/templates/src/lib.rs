use fframes::{Color, Duration, Frame, Styles, StylesError, Svgr, Typography, Video};

pub struct StudioVideo {
    background: Color,
    text: Color,
    title: Typography,
    margin: f32,
}

impl StudioVideo {
    /// Parses the embedded design tokens and resolves every token the video uses, once.
    /// `render_frame` only reads the resolved values, so it never fails or parses.
    pub fn new() -> Result<Self, StylesError> {
        let styles = Styles::from_json_str(include_str!("../style/tokens.json"))?;
        Ok(Self {
            background: styles.color("color.background")?,
            text: styles.color("color.text")?,
            title: styles.typography("typography.title")?.clone(),
            margin: styles.dimension("spacing.margin")?,
        })
    }
}

impl Video for StudioVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> { Duration::Seconds(5.0) }
    fn audio(&self) -> fframes::AudioMap<'_> { fframes::AudioMap::none() }
    fn render_frame<'a>(&'a self, _frame: Frame, _ctx: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        let (background, text, title, margin) = (self.background, self.text, &self.title, self.margin);
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width={Self::WIDTH} height={Self::HEIGHT}>
                <rect width="1920" height="1080" fill={background} />
                <text x={margin} y="300" font-family={title.family.as_str()} font-size={title.size} font-weight={title.weight} fill={text}>
                    "Your video starts here"
                </text>
            </svg>
        )
    }
}
