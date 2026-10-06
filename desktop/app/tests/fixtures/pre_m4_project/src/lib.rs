use fframes::{Color, Duration, Frame, Svgr, Video};

pub struct StudioVideo;

impl Video for StudioVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 1920;
    const HEIGHT: usize = 1080;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> { Duration::Seconds(5.0) }
    fn audio(&self) -> fframes::AudioMap<'_> { fframes::AudioMap::none() }
    fn render_frame<'a>(&'a self, _frame: Frame, _ctx: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1920 1080" width={Self::WIDTH} height={Self::HEIGHT}>
                <rect width="1920" height="1080" fill="#0d1117" />
                <text x="100" y="300" font-family="DM Sans" font-size="120" fill="#ffffff">
                    "Your video starts here"
                </text>
            </svg>
        )
    }
}
