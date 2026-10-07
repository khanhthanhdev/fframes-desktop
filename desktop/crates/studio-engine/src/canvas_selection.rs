//! GPUI-free mapping and hit testing for the exact displayed preview frame.
use fframes_studio_protocol::{
    EditorFrameMetadata, EditorFrameStatus, EditorGeometrySupport, EditorObjectGeometry,
    EditorObjectIdentity, PreviewIdentity, PreviewTimelineResponse,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl CanvasRect {
    fn valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Letterbox, zoom and pan in the same logical-coordinate transform used to paint pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasViewport {
    bounds: CanvasRect,
    video_width: u32,
    video_height: u32,
    fit_scale: f64,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanvasSelectionError {
    #[error("invalid canvas viewport or video dimensions")]
    InvalidViewport,
    #[error("editor metadata does not match the displayed frame")]
    StaleFrame,
    #[error("editor metadata is invalid or unavailable")]
    InvalidMetadata,
    #[error("selected object is not present in matching frame metadata")]
    MissingObject,
}

impl CanvasViewport {
    pub fn new(
        bounds: CanvasRect,
        video_width: u32,
        video_height: u32,
    ) -> Result<Self, CanvasSelectionError> {
        if !bounds.valid() || video_width == 0 || video_height == 0 {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        let fit_scale =
            (bounds.width / f64::from(video_width)).min(bounds.height / f64::from(video_height));
        if !fit_scale.is_finite() || fit_scale <= 0.0 {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        Ok(Self {
            bounds,
            video_width,
            video_height,
            fit_scale,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        })
    }

    pub fn resize(&mut self, bounds: CanvasRect) -> Result<(), CanvasSelectionError> {
        if !bounds.valid() {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        let fit_scale = (bounds.width / f64::from(self.video_width))
            .min(bounds.height / f64::from(self.video_height));
        if !fit_scale.is_finite() || fit_scale <= 0.0 {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        self.bounds = bounds;
        self.fit_scale = fit_scale;
        Ok(())
    }

    pub fn resize_if_changed(&mut self, bounds: CanvasRect) -> Result<(), CanvasSelectionError> {
        if self.bounds == bounds {
            Ok(())
        } else {
            self.resize(bounds)
        }
    }

    pub fn video_dimensions(&self) -> (u32, u32) {
        (self.video_width, self.video_height)
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn reset_to_fit(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }

    pub fn pan_by(&mut self, delta_x: f64, delta_y: f64) -> Result<(), CanvasSelectionError> {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        self.pan_x += delta_x;
        self.pan_y += delta_y;
        Ok(())
    }

    /// Zoom around the pointer without moving the video point under it.
    pub fn zoom_at(&mut self, x: f64, y: f64, factor: f64) -> Result<(), CanvasSelectionError> {
        if !x.is_finite() || !y.is_finite() || !factor.is_finite() || factor <= 0.0 {
            return Err(CanvasSelectionError::InvalidViewport);
        }
        let anchor = self.point_to_video_unclipped(x, y);
        let next_zoom = (self.zoom * factor).clamp(1.0, 32.0);
        let (origin_x, origin_y) = self.fit_origin();
        self.zoom = next_zoom;
        let scale = self.fit_scale * self.zoom;
        self.pan_x = x - origin_x - anchor.x * scale;
        self.pan_y = y - origin_y - anchor.y * scale;
        Ok(())
    }

    pub fn image_bounds(&self) -> CanvasRect {
        let (origin_x, origin_y) = self.fit_origin();
        let scale = self.fit_scale * self.zoom;
        CanvasRect {
            x: origin_x + self.pan_x,
            y: origin_y + self.pan_y,
            width: f64::from(self.video_width) * scale,
            height: f64::from(self.video_height) * scale,
        }
    }

    pub fn video_rect_bounds(&self, rect: &fframes_studio_protocol::Rect) -> CanvasRect {
        let (origin_x, origin_y) = self.fit_origin();
        let scale = self.fit_scale * self.zoom;
        CanvasRect {
            x: origin_x + self.pan_x + f64::from(rect.x) * scale,
            y: origin_y + self.pan_y + f64::from(rect.y) * scale,
            width: f64::from(rect.width) * scale,
            height: f64::from(rect.height) * scale,
        }
    }

    pub fn video_scope_bounds(&self, rect: &VideoRect) -> CanvasRect {
        let (origin_x, origin_y) = self.fit_origin();
        let scale = self.fit_scale * self.zoom;
        CanvasRect {
            x: origin_x + self.pan_x + rect.x * scale,
            y: origin_y + self.pan_y + rect.y * scale,
            width: rect.width * scale,
            height: rect.height * scale,
        }
    }

    pub fn point_to_video(&self, x: f64, y: f64) -> Option<VideoPoint> {
        if !self.bounds.contains(x, y) || !self.image_bounds().contains(x, y) {
            return None;
        }
        let point = self.point_to_video_unclipped(x, y);
        (point.x >= 0.0
            && point.y >= 0.0
            && point.x < f64::from(self.video_width)
            && point.y < f64::from(self.video_height))
        .then_some(point)
    }

    /// Convert a drag endpoint to video coordinates, clamped to the painted image.
    pub fn point_to_video_clamped(&self, x: f64, y: f64) -> Option<VideoPoint> {
        if !x.is_finite() || !y.is_finite() || !self.bounds.contains(x, y) {
            return None;
        }
        let point = self.point_to_video_unclipped(x, y);
        Some(VideoPoint {
            x: point.x.clamp(0.0, f64::from(self.video_width)),
            y: point.y.clamp(0.0, f64::from(self.video_height)),
        })
    }

    pub fn rectangle_from_drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
    ) -> Option<VideoRect> {
        let start = self.point_to_video(start_x, start_y)?;
        let end = self.point_to_video_clamped(end_x, end_y)?;
        let x = start.x.min(end.x);
        let y = start.y.min(end.y);
        let right = start.x.max(end.x);
        let bottom = start.y.max(end.y);
        (right > x && bottom > y).then_some(VideoRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }

    fn fit_origin(&self) -> (f64, f64) {
        (
            self.bounds.x
                + (self.bounds.width - f64::from(self.video_width) * self.fit_scale) / 2.0,
            self.bounds.y
                + (self.bounds.height - f64::from(self.video_height) * self.fit_scale) / 2.0,
        )
    }

    fn point_to_video_unclipped(&self, x: f64, y: f64) -> VideoPoint {
        let (origin_x, origin_y) = self.fit_origin();
        let scale = self.fit_scale * self.zoom;
        VideoPoint {
            x: (x - origin_x - self.pan_x) / scale,
            y: (y - origin_y - self.pan_y) / scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayedFrameTag {
    pub preview: PreviewIdentity,
    pub frame_index: usize,
    pub seek_serial: u64,
    pub frame_geometry_digest: String,
}

impl DisplayedFrameTag {
    pub fn from_metadata(
        preview: PreviewIdentity,
        metadata: &EditorFrameMetadata,
    ) -> Result<Self, CanvasSelectionError> {
        metadata
            .validate_for_frame(metadata.frame_index, metadata.seek_serial)
            .map_err(|_| CanvasSelectionError::InvalidMetadata)?;
        Ok(Self {
            preview,
            frame_index: metadata.frame_index,
            seek_serial: metadata.seek_serial,
            frame_geometry_digest: metadata.frame_geometry_digest.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanvasSelection {
    pub identity: EditorObjectIdentity,
    pub source_anchor: Option<fframes_studio_protocol::EditorSourceAnchor>,
    pub style_tokens: Vec<String>,
    pub bounds: fframes_studio_protocol::Rect,
    pub support: EditorGeometrySupport,
    pub displayed: DisplayedFrameTag,
}

fn candidates_at<'a>(
    metadata: &'a EditorFrameMetadata,
    displayed: &DisplayedFrameTag,
    point: VideoPoint,
) -> Result<Vec<&'a EditorObjectGeometry>, CanvasSelectionError> {
    if metadata.frame_index != displayed.frame_index
        || metadata.seek_serial != displayed.seek_serial
        || metadata.frame_geometry_digest != displayed.frame_geometry_digest
    {
        return Err(CanvasSelectionError::StaleFrame);
    }
    metadata
        .validate_for_frame(displayed.frame_index, displayed.seek_serial)
        .map_err(|_| CanvasSelectionError::InvalidMetadata)?;
    if metadata.status != EditorFrameStatus::Supported {
        return Ok(Vec::new());
    }
    let mut candidates: Vec<_> = metadata
        .objects
        .iter()
        .filter(|object| {
            object.support != EditorGeometrySupport::Unsupported
                && point.x >= f64::from(object.bounds.x)
                && point.y >= f64::from(object.bounds.y)
                && point.x < f64::from(object.bounds.x + object.bounds.width)
                && point.y < f64::from(object.bounds.y + object.bounds.height)
        })
        .collect();
    let parent_by_identity: std::collections::HashMap<_, _> = metadata
        .objects
        .iter()
        .map(|object| (&object.identity, object.parent.as_ref()))
        .collect();
    let depths: std::collections::HashMap<_, _> = metadata
        .objects
        .iter()
        .map(|object| {
            let mut depth = 0;
            let mut parent = object.parent.as_ref();
            while let Some(identity) = parent {
                depth += 1;
                parent = parent_by_identity.get(identity).and_then(|parent| *parent);
            }
            (&object.identity, depth)
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.paint_order
            .cmp(&a.paint_order)
            .then_with(|| depths.get(&&b.identity).cmp(&depths.get(&&a.identity)))
            .then_with(|| identity_sort_key(&a.identity).cmp(&identity_sort_key(&b.identity)))
    });
    Ok(candidates)
}

fn identity_sort_key(identity: &EditorObjectIdentity) -> (&str, &str, &str, &str) {
    (
        &identity.scene_instance_key,
        &identity.component_key,
        &identity.object_key,
        &identity.repeat_key,
    )
}

pub fn hit_test(
    metadata: &EditorFrameMetadata,
    displayed: &DisplayedFrameTag,
    viewport: &CanvasViewport,
    x: f64,
    y: f64,
) -> Result<Option<CanvasSelection>, CanvasSelectionError> {
    if metadata.video_width != viewport.video_width
        || metadata.video_height != viewport.video_height
    {
        return Err(CanvasSelectionError::InvalidViewport);
    }
    let Some(point) = viewport.point_to_video(x, y) else {
        return Ok(None);
    };
    let candidates = candidates_at(metadata, displayed, point)?;
    let leaf = candidates.iter().find(|candidate| {
        !candidates
            .iter()
            .any(|other| other.parent.as_ref() == Some(&candidate.identity))
    });
    let Some(object) = leaf.or_else(|| candidates.first()) else {
        return Ok(None);
    };
    Ok(Some(CanvasSelection {
        identity: object.identity.clone(),
        source_anchor: object.source_anchor.clone(),
        style_tokens: object.style_tokens.clone(),
        bounds: object.bounds.clone(),
        support: object.support,
        displayed: displayed.clone(),
    }))
}

pub fn cycle_hit_candidates(
    metadata: &EditorFrameMetadata,
    displayed: &DisplayedFrameTag,
    viewport: &CanvasViewport,
    x: f64,
    y: f64,
    current: Option<&EditorObjectIdentity>,
) -> Result<Option<CanvasSelection>, CanvasSelectionError> {
    if metadata.video_width != viewport.video_width
        || metadata.video_height != viewport.video_height
    {
        return Err(CanvasSelectionError::InvalidViewport);
    }
    let Some(point) = viewport.point_to_video(x, y) else {
        return Ok(None);
    };
    let candidates = candidates_at(metadata, displayed, point)?;
    if candidates.is_empty() {
        return Ok(None);
    }
    let next = current
        .and_then(|current| {
            candidates
                .iter()
                .position(|object| &object.identity == current)
        })
        .map_or(0, |index| (index + 1) % candidates.len());
    let object = candidates[next];
    Ok(Some(CanvasSelection {
        identity: object.identity.clone(),
        source_anchor: object.source_anchor.clone(),
        style_tokens: object.style_tokens.clone(),
        bounds: object.bounds.clone(),
        support: object.support,
        displayed: displayed.clone(),
    }))
}

/// A semantic selection crosses frames only if the same full key is present under
/// the same preview identity. It is never remapped by name, paint order or bounds.
pub fn revalidate_selection(
    selection: &CanvasSelection,
    new_displayed: &DisplayedFrameTag,
    metadata: &EditorFrameMetadata,
) -> Result<CanvasSelection, CanvasSelectionError> {
    if selection.displayed.preview != new_displayed.preview {
        return Err(CanvasSelectionError::StaleFrame);
    }
    if metadata.frame_index != new_displayed.frame_index
        || metadata.seek_serial != new_displayed.seek_serial
        || metadata.frame_geometry_digest != new_displayed.frame_geometry_digest
    {
        return Err(CanvasSelectionError::StaleFrame);
    }
    metadata
        .validate_for_frame(new_displayed.frame_index, new_displayed.seek_serial)
        .map_err(|_| CanvasSelectionError::InvalidMetadata)?;
    let object = metadata
        .objects
        .iter()
        .find(|object| {
            object.identity == selection.identity
                && object.support != EditorGeometrySupport::Unsupported
        })
        .ok_or(CanvasSelectionError::MissingObject)?;
    Ok(CanvasSelection {
        identity: object.identity.clone(),
        source_anchor: object.source_anchor.clone(),
        style_tokens: object.style_tokens.clone(),
        bounds: object.bounds.clone(),
        support: object.support,
        displayed: new_displayed.clone(),
    })
}

/// Scene choices at an overlap remain explicit instead of guessing a winner.
pub fn active_scenes_at(
    timeline: &PreviewTimelineResponse,
    frame_index: usize,
) -> Vec<&fframes_studio_protocol::PreviewSceneInfo> {
    timeline
        .scenes
        .iter()
        .filter(|scene| scene.start_frame <= frame_index && frame_index < scene.end_frame)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fframes_studio_protocol::{EditorFrameStatus, Rect};

    fn identity() -> PreviewIdentity {
        PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "a".repeat(64),
            worker_generation: 2,
        }
    }

    fn object(key: &str, parent: Option<&str>, bounds: Rect, order: u32) -> EditorObjectGeometry {
        let id = |key: &str| EditorObjectIdentity {
            scene_instance_key: "scene".into(),
            component_key: "title".into(),
            object_key: key.into(),
            repeat_key: "primary".into(),
        };
        EditorObjectGeometry {
            identity: id(key),
            parent: parent.map(id),
            source_anchor: None,
            style_tokens: Vec::new(),
            bounds,
            paint_order: order,
            support: EditorGeometrySupport::ExactBounds,
        }
    }

    fn metadata() -> EditorFrameMetadata {
        EditorFrameMetadata {
            frame_index: 12,
            seek_serial: 4,
            video_width: 1920,
            video_height: 1080,
            editor_index_digest: "a".repeat(64),
            frame_geometry_digest: "b".repeat(64),
            status: EditorFrameStatus::Supported,
            reason: None,
            objects: vec![
                object(
                    "group",
                    None,
                    Rect {
                        x: 100.0,
                        y: 100.0,
                        width: 500.0,
                        height: 300.0,
                    },
                    1,
                ),
                object(
                    "under",
                    Some("group"),
                    Rect {
                        x: 150.0,
                        y: 150.0,
                        width: 300.0,
                        height: 120.0,
                    },
                    2,
                ),
                object(
                    "top",
                    Some("group"),
                    Rect {
                        x: 200.0,
                        y: 180.0,
                        width: 100.0,
                        height: 80.0,
                    },
                    3,
                ),
            ],
        }
    }

    fn displayed(metadata: &EditorFrameMetadata) -> DisplayedFrameTag {
        DisplayedFrameTag::from_metadata(identity(), metadata).unwrap()
    }

    #[test]
    fn fit_letterbox_boundaries_and_pointer_centered_zoom_map_exactly() {
        let mut view = CanvasViewport::new(
            CanvasRect {
                x: 10.0,
                y: 20.0,
                width: 1000.0,
                height: 1000.0,
            },
            1920,
            1080,
        )
        .unwrap();
        let image = view.image_bounds();
        assert!((image.width - 1000.0).abs() < 0.001);
        assert!((image.height - 562.5).abs() < 0.001);
        assert!(view.point_to_video(500.0, 100.0).is_none());
        let pixel = view.point_to_video(510.0, 520.0).unwrap();
        assert!((pixel.x - 960.0).abs() < 0.001);
        assert!((pixel.y - 540.0).abs() < 0.001);
        let anchor = (600.0, 350.0);
        let before = view.point_to_video(anchor.0, anchor.1).unwrap();
        view.zoom_at(anchor.0, anchor.1, 2.0).unwrap();
        let after = view.point_to_video(anchor.0, anchor.1).unwrap();
        assert!((before.x - after.x).abs() < 0.001);
        assert!((before.y - after.y).abs() < 0.001);
        view.pan_by(22.0, -13.0).unwrap();
        assert_ne!(view.point_to_video(anchor.0, anchor.1), Some(before));
        view.reset_to_fit();
        assert_eq!(view.zoom(), 1.0);
    }

    #[test]
    fn topmost_leaf_cycles_through_overlap_and_rejects_stale_revision() {
        let metadata = metadata();
        let displayed = displayed(&metadata);
        let view = CanvasViewport::new(
            CanvasRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            1920,
            1080,
        )
        .unwrap();
        let top = hit_test(&metadata, &displayed, &view, 220.0, 200.0)
            .unwrap()
            .unwrap();
        assert_eq!(top.identity.object_key, "top");
        let under = cycle_hit_candidates(
            &metadata,
            &displayed,
            &view,
            220.0,
            200.0,
            Some(&top.identity),
        )
        .unwrap()
        .unwrap();
        assert_eq!(under.identity.object_key, "under");
        let group = cycle_hit_candidates(
            &metadata,
            &displayed,
            &view,
            220.0,
            200.0,
            Some(&under.identity),
        )
        .unwrap()
        .unwrap();
        assert_eq!(group.identity.object_key, "group");

        let mut changed_revision = displayed.clone();
        changed_revision.preview.source_revision = "c".repeat(64);
        assert_eq!(
            revalidate_selection(&top, &changed_revision, &metadata),
            Err(CanvasSelectionError::StaleFrame)
        );
    }

    #[test]
    fn revalidation_does_not_keep_an_object_that_became_unsupported() {
        let original = metadata();
        let original_tag = displayed(&original);
        let viewport = CanvasViewport::new(
            CanvasRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            1920,
            1080,
        )
        .unwrap();
        let selected = hit_test(&original, &original_tag, &viewport, 220.0, 200.0)
            .unwrap()
            .unwrap();
        assert_eq!(selected.identity.object_key, "top");

        let mut next = metadata();
        next.frame_index = 13;
        next.seek_serial = 5;
        next.frame_geometry_digest = "c".repeat(64);
        next.objects
            .iter_mut()
            .find(|object| object.identity.object_key == "top")
            .unwrap()
            .support = EditorGeometrySupport::Unsupported;
        let next_tag = displayed(&next);
        assert_eq!(
            revalidate_selection(&selected, &next_tag, &next),
            Err(CanvasSelectionError::MissingObject)
        );
    }

    #[test]
    fn rectangle_scope_clamps_to_full_resolution_video_pixels() {
        let view = CanvasViewport::new(
            CanvasRect {
                x: 0.0,
                y: 0.0,
                width: 960.0,
                height: 540.0,
            },
            1920,
            1080,
        )
        .unwrap();
        let rect = view
            .rectangle_from_drag(100.0, 100.0, 900.0, 500.0)
            .unwrap();
        assert_eq!(
            rect,
            VideoRect {
                x: 200.0,
                y: 200.0,
                width: 1600.0,
                height: 800.0,
            }
        );
        assert!(
            view.rectangle_from_drag(300.0, 200.0, 300.0, 200.0)
                .is_none()
        );
    }
}
