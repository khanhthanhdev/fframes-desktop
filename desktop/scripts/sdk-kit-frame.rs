//! SDK qualification: an editable fframes-kit component drawn through fframes-studio-kit's
//! props store, built offline from the SDK's vendored kit. `assemble-phase-zero-sdk.py`
//! copies this file into the qualification worker as `examples/kit_frame.rs` and renders
//! `frame 0` with it.
use std::sync::Arc;

use fframes::{RenderOptions, cli};
use fframes_kit::prelude::*;
use fframes_studio_kit::PropsStore;

#[derive(Debug)]
struct KitFrame {
    theme: Theme,
    store: Arc<PropsStore>,
}

impl Video for KitFrame {
    const FPS: usize = 30;
    const WIDTH: usize = 640;
    const HEIGHT: usize = 360;

    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(1.0)
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let mut ui = Ui::new(frame, ctx, &self.theme).with_props(self.store.view());
        let dot = ui!(<Circle editable="dot" cx={320.0} cy={180.0} radius={40.0} />);
        svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360" width="640" height="360">
                <rect x="0" y="0" width="640" height="360" fill="#000000" />
                {dot}
            </svg>
        )
    }
}

fn main() -> std::process::ExitCode {
    let video = KitFrame {
        theme: Theme::default(),
        store: Arc::new(PropsStore::default()),
    };
    cli::new(&video, RenderOptions::default()).run()
}
