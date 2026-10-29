//! Displayed-frame bundle and viewport state for semantic canvas selection.
use fframes_studio_protocol::{EditorFrameMetadata, EditorFrameStatus, EditorGeometrySupport};
use fframes_studio_protocol::{EditorObjectIdentity, PreviewIdentity};
use studio_engine::{
    CanvasRect, CanvasSelection, CanvasSelectionError, CanvasTaskSelection,
    CanvasTaskSelectionKind, CanvasViewport, DisplayedFrameTag, PreviewFrame, VideoPixelRect,
    VideoRect, cycle_hit_candidates, hit_test, revalidate_selection,
};

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayedCanvasFrame {
    pub preview: PreviewIdentity,
    pub frame_index: usize,
    pub seek_serial: u64,
    pub metadata: Option<EditorFrameMetadata>,
    pub index_digest: Option<String>,
}

impl DisplayedCanvasFrame {
    fn tag(&self) -> Result<DisplayedFrameTag, CanvasSelectionError> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or(CanvasSelectionError::InvalidMetadata)?;
        DisplayedFrameTag::from_metadata(self.preview.clone(), metadata)
    }
}

#[derive(Debug, Default)]
pub struct CanvasViewState {
    pub viewport: Option<CanvasViewport>,
    pub displayed: Option<DisplayedCanvasFrame>,
    pub selection: Option<CanvasSelection>,
    pub rectangle_scope: Option<VideoRect>,
    pub message: Option<String>,
    /// Why the last canvas action failed. Unlike message it stays until dismissed or the
    /// selection is cleared.
    pub error: Option<String>,
    rectangle_start: Option<(f64, f64)>,
    last_click: Option<(f64, f64)>,
}

impl CanvasViewState {
    /// Keep the geometry coupled to the exact accepted frame. Invalid optional metadata
    /// disables object selection without discarding otherwise valid preview pixels.
    pub fn install_frame(
        &mut self,
        preview: PreviewIdentity,
        frame: &PreviewFrame,
    ) -> Result<(), CanvasSelectionError> {
        frame
            .validate(&preview)
            .map_err(|_| CanvasSelectionError::InvalidMetadata)?;
        let metadata = frame.response.editor_metadata.clone();
        let index_digest = metadata
            .as_ref()
            .map(|metadata| metadata.editor_index_digest.clone());
        let next = DisplayedCanvasFrame {
            preview,
            frame_index: frame.response.frame_index,
            seek_serial: frame.response.seek_serial,
            metadata,
            index_digest,
        };

        let revalidated = self.selection.as_ref().and_then(|selection| {
            let tag = next.tag().ok()?;
            let metadata = next.metadata.as_ref()?;
            revalidate_selection(selection, &tag, metadata).ok()
        });
        let selection_was_lost = self.selection.is_some() && revalidated.is_none();
        self.selection = revalidated;
        self.rectangle_scope = None;
        self.rectangle_start = None;
        self.displayed = Some(next);
        self.message = if selection_was_lost {
            Some(
                "Canvas selection cleared because it was not present in the displayed frame."
                    .into(),
            )
        } else {
            match self
                .displayed
                .as_ref()
                .and_then(|displayed| displayed.metadata.as_ref())
            {
                None => {
                    Some("Semantic canvas selection is unavailable for this preview worker.".into())
                }
                Some(metadata) if metadata.status == EditorFrameStatus::Unannotated => Some(
                    "No semantic canvas objects are registered; choose a scene or rectangle scope."
                        .into(),
                ),
                Some(metadata) if metadata.status == EditorFrameStatus::Invalid => {
                    Some(metadata.reason.clone().unwrap_or_else(|| {
                        "Canvas metadata is invalid; scene/range selection remains available."
                            .into()
                    }))
                }
                _ => None,
            }
        };
        Ok(())
    }

    pub fn clear(&mut self) {
        self.displayed = None;
        self.selection = None;
        self.rectangle_scope = None;
        self.rectangle_start = None;
        self.last_click = None;
        self.message = None;
        self.error = None;
    }

    pub fn resize(
        &mut self,
        width: f64,
        height: f64,
        video_width: u32,
        video_height: u32,
    ) -> Result<(), CanvasSelectionError> {
        let bounds = CanvasRect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        match &mut self.viewport {
            Some(viewport) if viewport.video_dimensions() == (video_width, video_height) => {
                viewport.resize_if_changed(bounds)
            }
            Some(_) => {
                self.viewport = Some(CanvasViewport::new(bounds, video_width, video_height)?);
                Ok(())
            }
            None => {
                self.viewport = Some(CanvasViewport::new(bounds, video_width, video_height)?);
                Ok(())
            }
        }
    }

    pub fn select_at(
        &mut self,
        x: f64,
        y: f64,
        cycle: bool,
    ) -> Result<Option<&CanvasSelection>, CanvasSelectionError> {
        let displayed = self
            .displayed
            .as_ref()
            .ok_or(CanvasSelectionError::InvalidMetadata)?;
        let metadata = displayed
            .metadata
            .as_ref()
            .ok_or(CanvasSelectionError::InvalidMetadata)?;
        let tag = displayed.tag()?;
        let viewport = self
            .viewport
            .as_ref()
            .ok_or(CanvasSelectionError::InvalidViewport)?;
        if metadata.status != EditorFrameStatus::Supported {
            self.last_click = Some((x, y));
            self.rectangle_scope = None;
            self.selection = None;
            self.message = Some(match metadata.status {
                EditorFrameStatus::Unannotated => {
                    "No semantic canvas objects are registered; choose a scene or rectangle scope."
                        .into()
                }
                EditorFrameStatus::Invalid => metadata.reason.clone().unwrap_or_else(|| {
                    "Canvas metadata is invalid; scene/range selection remains available.".into()
                }),
                EditorFrameStatus::Supported => unreachable!(),
            });
            return Ok(None);
        }
        let next = if cycle {
            let current = self.selection.as_ref().map(|selection| &selection.identity);
            cycle_hit_candidates(metadata, &tag, viewport, x, y, current)?
        } else {
            hit_test(metadata, &tag, viewport, x, y)?
        };
        self.last_click = Some((x, y));
        self.rectangle_scope = None;
        self.selection = next;
        self.message = self.selection.as_ref().map(|selection| {
            if selection.support == EditorGeometrySupport::ApproximateBounds {
                "Approximate bounds · selection is a bounding box, not an exact shape.".into()
            } else {
                format!(
                    "Selected {} / {}",
                    selection.identity.component_key, selection.identity.object_key
                )
            }
        });
        Ok(self.selection.as_ref())
    }

    pub fn cycle_last_hit(&mut self) -> Result<Option<&CanvasSelection>, CanvasSelectionError> {
        let Some((x, y)) = self.last_click else {
            return Ok(self.selection.as_ref());
        };
        self.select_at(x, y, true)
    }

    pub fn selected_bounds(&self) -> Option<CanvasRect> {
        Some(
            self.viewport
                .as_ref()?
                .video_rect_bounds(&self.selection.as_ref()?.bounds),
        )
    }

    pub fn begin_rectangle(&mut self, x: f64, y: f64) -> bool {
        let Some(viewport) = &self.viewport else {
            return false;
        };
        if viewport.point_to_video(x, y).is_none() {
            return false;
        }
        self.rectangle_start = Some((x, y));
        self.rectangle_scope = None;
        self.selection = None;
        true
    }

    pub fn update_rectangle(&mut self, x: f64, y: f64) -> Option<VideoRect> {
        let (start_x, start_y) = self.rectangle_start?;
        self.rectangle_scope = self
            .viewport
            .as_ref()?
            .rectangle_from_drag(start_x, start_y, x, y);
        self.rectangle_scope
    }

    pub fn end_rectangle(&mut self) -> Option<VideoRect> {
        self.rectangle_start = None;
        self.rectangle_scope
    }

    pub fn is_drawing_rectangle(&self) -> bool {
        self.rectangle_start.is_some()
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.rectangle_scope = None;
        self.rectangle_start = None;
        self.message = None;
        self.error = None;
    }

    pub fn selection_identity(&self) -> Option<&EditorObjectIdentity> {
        self.selection.as_ref().map(|selection| &selection.identity)
    }

    pub fn task_scope_selection(&self) -> Option<CanvasTaskSelection> {
        let displayed = self.displayed.as_ref()?;
        let selection = if let Some(selection) = &self.selection {
            let metadata = displayed.metadata.as_ref()?;
            CanvasTaskSelectionKind::Element {
                identity: selection.identity.clone(),
                bounds: video_pixel_rect(
                    selection.bounds.x,
                    selection.bounds.y,
                    selection.bounds.width,
                    selection.bounds.height,
                    metadata.video_width,
                    metadata.video_height,
                )?,
                support: selection.support,
                source_anchor: selection.source_anchor.clone(),
                style_tokens: selection.style_tokens.clone(),
            }
        } else if let Some(rectangle) = &self.rectangle_scope {
            let (video_width, video_height) = self.viewport.as_ref()?.video_dimensions();
            CanvasTaskSelectionKind::Rectangle {
                bounds: video_pixel_rect(
                    rectangle.x as f32,
                    rectangle.y as f32,
                    rectangle.width as f32,
                    rectangle.height as f32,
                    video_width,
                    video_height,
                )?,
            }
        } else {
            return None;
        };
        let (video_width, video_height) = displayed
            .metadata
            .as_ref()
            .map(|metadata| (metadata.video_width, metadata.video_height))
            .or_else(|| self.viewport.as_ref().map(CanvasViewport::video_dimensions))?;
        Some(CanvasTaskSelection {
            preview: displayed.preview.clone(),
            frame_index: displayed.frame_index,
            seek_serial: displayed.seek_serial,
            editor_index_digest: displayed
                .metadata
                .as_ref()
                .map(|metadata| metadata.editor_index_digest.clone()),
            frame_geometry_digest: displayed
                .metadata
                .as_ref()
                .map(|metadata| metadata.frame_geometry_digest.clone()),
            video_width,
            video_height,
            selection,
        })
    }
}

fn video_pixel_rect(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    video_width: u32,
    video_height: u32,
) -> Option<VideoPixelRect> {
    if ![x, y, width, height].iter().all(|value| value.is_finite()) || width <= 0.0 || height <= 0.0
    {
        return None;
    }
    let right = (x + width).ceil().max(0.0).min(video_width as f32) as u32;
    let bottom = (y + height).ceil().max(0.0).min(video_height as f32) as u32;
    let x = x.floor().max(0.0).min(video_width as f32) as u32;
    let y = y.floor().max(0.0).min(video_height as f32) as u32;
    (right > x && bottom > y).then_some(VideoPixelRect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

/// Where the video frame sits inside the preview surface and how many surface
/// pixels one video pixel covers. Painting, the selection and scope overlays and
/// click mapping all read this one geometry, so they cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageLayout {
    pub frame_rect: CanvasRect,
    pub scale: f64,
}

impl StageLayout {
    /// The geometry of a viewport, including its current zoom and pan.
    pub fn from_viewport(viewport: &CanvasViewport) -> Self {
        let frame_rect = viewport.image_bounds();
        let (video_width, _) = viewport.video_dimensions();
        Self {
            frame_rect,
            scale: frame_rect.width / f64::from(video_width),
        }
    }

    /// Moves a rectangle in surface coordinates into frame coordinates.
    pub fn to_frame_space(&self, rect: CanvasRect) -> CanvasRect {
        CanvasRect {
            x: rect.x - self.frame_rect.x,
            y: rect.y - self.frame_rect.y,
            ..rect
        }
    }
}

/// The letterboxed frame centred in `bounds` at fit zoom. `None` when the bounds
/// or the frame size are empty.
pub fn stage_layout(bounds: CanvasRect, frame_size: (u32, u32)) -> Option<StageLayout> {
    CanvasViewport::new(bounds, frame_size.0, frame_size.1)
        .ok()
        .map(|viewport| StageLayout::from_viewport(&viewport))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(width: f64, height: f64) -> CanvasRect {
        CanvasRect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    #[test]
    fn stage_layout_letterboxes_and_never_upscales() {
        // A 16:9 frame in a wide surface is centred with bars left and right.
        let layout = stage_layout(bounds(1000.0, 400.0), (1920, 1080)).unwrap();
        assert!((layout.frame_rect.height - 400.0).abs() < 1e-9);
        assert!((layout.frame_rect.width - 400.0 * 16.0 / 9.0).abs() < 1e-9);
        let left = layout.frame_rect.x;
        let right = 1000.0 - (layout.frame_rect.x + layout.frame_rect.width);
        assert!((left - right).abs() < 1e-9 && left > 0.0);
        assert_eq!(layout.frame_rect.y, 0.0);
        // preview_scale (unchanged) never upscales and caps at 1280 x 720.
        let capped = crate::frame_image::preview_scale(1920, 1080, (3840, 2160));
        assert!((capped - 1280.0 / 1920.0).abs() < 1e-9);
        assert_eq!(
            crate::frame_image::preview_scale(640, 360, (3840, 2160)),
            1.0
        );
        let small = crate::frame_image::preview_scale(1920, 1080, (960, 540));
        assert!((small - 0.5).abs() < 1e-9);
        assert!((small - 0.5).abs() < 1e-9);
        assert!(stage_layout(bounds(0.0, 10.0), (1920, 1080)).is_none());
        assert!(stage_layout(bounds(10.0, 10.0), (0, 1080)).is_none());
    }

    #[test]
    fn stage_layout_round_trips_click_points() {
        let surface = bounds(731.0, 412.0);
        let viewport = CanvasViewport::new(surface, 1280, 720).unwrap();
        let layout = StageLayout::from_viewport(&viewport);
        for (vx, vy) in [(0.5, 0.5), (640.0, 360.0), (1279.0, 719.0), (100.25, 33.75)] {
            let wx = layout.frame_rect.x + vx * layout.scale;
            let wy = layout.frame_rect.y + vy * layout.scale;
            let back = viewport.point_to_video(wx, wy).expect("inside the frame");
            assert!((back.x - vx).abs() < 1e-6 && (back.y - vy).abs() < 1e-6);
        }
        // Outside the letterboxed frame there is nothing to hit.
        let outside = layout.frame_rect.x - 1.0;
        assert!(viewport.point_to_video(outside, 10.0).is_none());
    }

    #[test]
    fn stage_layout_follows_zoom_and_frame_space_is_relative() {
        let mut viewport = CanvasViewport::new(bounds(800.0, 450.0), 1600, 900).unwrap();
        viewport.zoom_at(400.0, 225.0, 2.0).unwrap();
        let layout = StageLayout::from_viewport(&viewport);
        assert!((layout.scale - 1.0).abs() < 1e-9);
        let in_frame = layout.to_frame_space(CanvasRect {
            x: layout.frame_rect.x + 10.0,
            y: layout.frame_rect.y + 20.0,
            width: 5.0,
            height: 6.0,
        });
        assert_eq!((in_frame.x, in_frame.y, in_frame.width), (10.0, 20.0, 5.0));
    }
}
