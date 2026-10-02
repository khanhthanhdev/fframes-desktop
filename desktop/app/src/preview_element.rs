use crate::app::StudioSpikeApp;
use gpui::{
    App, Bounds, Context, Element, ElementId, Entity, GlobalElementId, IntoElement, LayoutId,
    Pixels, RenderImage, Style, Window, point, px, relative, size,
};
use std::sync::Arc;

/// Paints the actual image, then acknowledges it on GPUI's next frame callback.
/// This observes GPUI presentation submission, not a physical-display GPU fence.
pub struct PreviewElement {
    pub app: Entity<StudioSpikeApp>,
    pub image: Option<Arc<RenderImage>>,
    pub dimensions: (u32, u32),
    pub completion: Option<(u64, usize)>,
}

impl IntoElement for PreviewElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for PreviewElement {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.app
            .update(cx, |app, _| app.preview_bounds = Some(bounds));
        let Some(image) = &self.image else {
            return;
        };
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }
        let scale = (f32::from(bounds.size.width) / self.dimensions.0 as f32)
            .min(f32::from(bounds.size.height) / self.dimensions.1 as f32);
        let image_size = size(
            px(self.dimensions.0 as f32 * scale),
            px(self.dimensions.1 as f32 * scale),
        );
        let image_bounds = Bounds::new(
            point(
                bounds.left() + (bounds.size.width - image_size.width) / 2.,
                bounds.top() + (bounds.size.height - image_size.height) / 2.,
            ),
            image_size,
        );
        match window.paint_image(
            bounds,
            image_bounds,
            Default::default(),
            image.clone(),
            0,
            false,
        ) {
            Ok(()) => {
                self.app.update(cx, |app, cx| {
                    if let Some(source) = app.pending_source.take() {
                        app.displayed_source = Some(source);
                    }
                    if self.completion.is_some_and(|token| token.1 == 0)
                        && let Some(output) = &app.qualification_output
                        && let Some(source) = &app.displayed_source
                        && let Some(title) = source
                            .elements
                            .iter()
                            .find(|element| element.element_id == "intro.title")
                        && let Some(input) = app.text_input.read(cx).last_bounds
                    {
                        // Native automation uses painted geometry, including the
                        // letterbox transform, rather than a timer or fixed pixels.
                        let readiness = serde_json::json!({
                            "generation": source.header.worker_generation,
                            "source_revision": source.header.source_revision,
                            "input_click": [f32::from(input.left()) + 10., f32::from(input.top()) + 10.],
                            "source_click": [
                                f32::from(image_bounds.left()) + (title.bounds.x + title.bounds.width.min(100.) / 2.) * scale,
                                f32::from(image_bounds.top()) + (title.bounds.y + title.bounds.height / 2.) * scale
                            ]
                        });
                        let ready = output.with_extension("ready.json");
                        let temporary = ready.with_extension("tmp");
                        if let Ok(bytes) = serde_json::to_vec(&readiness) {
                            let _ = std::fs::write(&temporary, bytes)
                                .and_then(|_| std::fs::rename(&temporary, &ready));
                        }
                    }
                });
                if let Some(token) = self.completion {
                    let should_schedule = self.app.update(cx, |app, _| {
                        if app.stress_painted == Some(token) {
                            false
                        } else {
                            app.stress_painted = Some(token);
                            app.stress_metrics.paint_submissions += 1;
                            true
                        }
                    });
                    if should_schedule {
                        let app = self.app.downgrade();
                        window.on_next_frame(move |_, cx| {
                            let _ = app.update(cx, |app, cx: &mut Context<StudioSpikeApp>| {
                                app.confirm_stress_frame(token, cx)
                            });
                        });
                    }
                }
            }
            Err(error) => self.app.update(cx, |app, cx| {
                app.abort_stress(format!("Image presentation failed: {error}"), cx);
                cx.notify();
            }),
        }
    }
}
