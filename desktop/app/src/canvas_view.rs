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
