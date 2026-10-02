use fframes_studio_protocol::{ElementMetadata, FrameHeader, Rect};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SelectedElementInfo {
    pub element_id: String,
    pub source_path: String,
    pub containing_symbol: String,
    pub byte_span: (usize, usize),
    pub code_snippet: String,
}

#[derive(Debug, Error, PartialEq)]
pub enum SelectionError {
    #[error("no element found at ({x}, {y})")]
    NoElementAtPoint { x: f32, y: f32 },
    #[error("stale metadata: generation {expected} != {actual}")]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("stale source revision: expected '{expected}', actual '{actual}'")]
    StaleRevision { expected: String, actual: String },
    #[error("source file '{path}' was modified on disk (hash mismatch)")]
    SourceModified { path: String },
    #[error("path escapes project root: {0}")]
    PathEscapesRoot(String),
    #[error("anchor markers missing or ambiguous: {0}")]
    MarkerMismatch(String),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Clone)]
pub struct SelectionSpike {
    pub source_revision: String,
    pub element_id: String,
    pub bounds: Rect,
    pub paint_order: u32,
    pub source_path: String,
    pub containing_symbol: String,
    pub source_hash: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub generation: u64,
}

impl SelectionSpike {
    pub fn from_metadata(metadata: &ElementMetadata, generation: u64) -> Self {
        Self {
            source_revision: metadata.source_revision.clone(),
            element_id: metadata.element_id.clone(),
            bounds: metadata.bounds.clone(),
            paint_order: metadata.paint_order,
            source_path: metadata.source_path.clone(),
            containing_symbol: metadata.containing_symbol.clone(),
            source_hash: metadata.source_hash.clone(),
            byte_start: metadata.byte_start,
            byte_end: metadata.byte_end,
            generation,
        }
    }

    pub fn new_title_fixture(
        workspace_root: &Path,
        rel_path: &str,
        generation: u64,
        source_revision: impl Into<String>,
    ) -> Result<Self, SelectionError> {
        let full_path = workspace_root.join(rel_path);
        let content =
            fs::read_to_string(&full_path).map_err(|e| SelectionError::Io(e.to_string()))?;

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let start_marker = "/* ANCHOR_START: intro.title */";
        let end_marker = "/* ANCHOR_END: intro.title */";
        let (start, end) = match (content.find(start_marker), content.find(end_marker)) {
            (Some(s), Some(e)) if s + start_marker.len() <= e => {
                let after_start = s + start_marker.len();
                let inner = &content[after_start..e];
                let leading = inner.len() - inner.trim_start().len();
                let trailing = inner.len() - inner.trim_end().len();
                (after_start + leading, e - trailing)
            }
            _ => return Err(SelectionError::MarkerMismatch(rel_path.into())),
        };

        Ok(Self {
            source_revision: source_revision.into(),
            element_id: "intro.title".into(),
            bounds: Rect {
                x: 100.0,
                y: 180.0,
                width: 1200.0,
                height: 150.0,
            },
            paint_order: 10,
            source_path: rel_path.to_string(),
            containing_symbol: "render_frame".to_string(),
            source_hash: hash,
            byte_start: start,
            byte_end: end,
            generation,
        })
    }

    /// Maps a click from viewport coordinates (including letterboxing) to video canvas pixels.
    pub fn map_viewport_to_canvas(
        viewport_w: f32,
        viewport_h: f32,
        canvas_w: f32,
        canvas_h: f32,
        click_x: f32,
        click_y: f32,
    ) -> Option<(f32, f32)> {
        if [viewport_w, viewport_h, canvas_w, canvas_h]
            .iter()
            .any(|n| !n.is_finite() || *n <= 0.)
            || !click_x.is_finite()
            || !click_y.is_finite()
        {
            return None;
        }
        let scale_x = viewport_w / canvas_w;
        let scale_y = viewport_h / canvas_h;
        let scale = scale_x.min(scale_y);

        let content_w = canvas_w * scale;
        let content_h = canvas_h * scale;

        let offset_x = (viewport_w - content_w) / 2.0;
        let offset_y = (viewport_h - content_h) / 2.0;

        if click_x < offset_x
            || click_x > offset_x + content_w
            || click_y < offset_y
            || click_y > offset_y + content_h
        {
            return None; // Clicked in letterbox black bars
        }

        let canvas_x = (click_x - offset_x) / scale;
        let canvas_y = (click_y - offset_y) / scale;

        Some((canvas_x, canvas_y))
    }

    /// Selects an element at point and validates its source anchor against disk.
    pub fn select_at_point(
        &self,
        workspace_root: &Path,
        x: f32,
        y: f32,
        current_gen: u64,
        current_revision: &str,
    ) -> Result<SelectedElementInfo, SelectionError> {
        if x < self.bounds.x
            || x > self.bounds.x + self.bounds.width
            || y < self.bounds.y
            || y > self.bounds.y + self.bounds.height
        {
            return Err(SelectionError::NoElementAtPoint { x, y });
        }

        if self.generation != current_gen {
            return Err(SelectionError::StaleGeneration {
                expected: current_gen,
                actual: self.generation,
            });
        }

        if self.source_revision != current_revision {
            return Err(SelectionError::StaleRevision {
                expected: current_revision.to_string(),
                actual: self.source_revision.clone(),
            });
        }
        let rel = Path::new(&self.source_path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(SelectionError::PathEscapesRoot(self.source_path.clone()));
        }
        let root = workspace_root
            .canonicalize()
            .map_err(|e| SelectionError::Io(e.to_string()))?;
        let full_path = root
            .join(rel)
            .canonicalize()
            .map_err(|e| SelectionError::Io(e.to_string()))?;
        if !full_path.starts_with(&root) {
            return Err(SelectionError::PathEscapesRoot(self.source_path.clone()));
        }
        let content =
            fs::read_to_string(&full_path).map_err(|e| SelectionError::Io(e.to_string()))?;

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let current_hash = format!("{:x}", hasher.finalize());

        if !current_hash.eq_ignore_ascii_case(&self.source_hash) {
            return Err(SelectionError::SourceModified {
                path: self.source_path.clone(),
            });
        }

        // Validate anchor markers are present and unambiguous
        let start_marker = format!("/* ANCHOR_START: {} */", self.element_id);
        let end_marker = format!("/* ANCHOR_END: {} */", self.element_id);
        let start_count = content.matches(&start_marker).count();
        let end_count = content.matches(&end_marker).count();
        if start_count != 1 || end_count != 1 {
            return Err(SelectionError::MarkerMismatch(format!(
                "expected 1 start and 1 end marker for '{}', found {} starts and {} ends",
                self.element_id, start_count, end_count
            )));
        }

        let marker_start = content.find(&start_marker).unwrap() + start_marker.len();
        let marker_end = content.find(&end_marker).unwrap();
        if self.byte_start < marker_start
            || self.byte_end > marker_end
            || self.byte_start >= self.byte_end
            || !content[..marker_start].contains(&format!("fn {}", self.containing_symbol))
        {
            return Err(SelectionError::MarkerMismatch(self.element_id.clone()));
        }
        // Validate UTF-8 character boundaries
        if !content.is_char_boundary(self.byte_start) || !content.is_char_boundary(self.byte_end) {
            return Err(SelectionError::Io(
                "span does not fall on valid UTF-8 character boundaries".into(),
            ));
        }

        let snippet = content
            .get(self.byte_start..self.byte_end)
            .ok_or_else(|| SelectionError::MarkerMismatch(self.element_id.clone()))?
            .to_string();
        Ok(SelectedElementInfo {
            element_id: self.element_id.clone(),
            source_path: self.source_path.clone(),
            containing_symbol: self.containing_symbol.clone(),
            byte_span: (self.byte_start, self.byte_end),
            code_snippet: snippet,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_letterbox_mapping() {
        assert!(SelectionSpike::map_viewport_to_canvas(0., 200., 1920., 1080., 1., 1.).is_none());
        assert!(
            SelectionSpike::map_viewport_to_canvas(400., 200., 1920., 1080., f32::NAN, 1.)
                .is_none()
        );
        // Viewport 400x200, Canvas 1920x1080 (16:9).
        // 16:9 in 400x200 -> content_w = 355.5, content_h = 200, offset_x ≈ 22.2, offset_y = 0.
        let click_in_bar =
            SelectionSpike::map_viewport_to_canvas(400.0, 200.0, 1920.0, 1080.0, 5.0, 100.0);
        assert!(click_in_bar.is_none());

        let center =
            SelectionSpike::map_viewport_to_canvas(400.0, 200.0, 1920.0, 1080.0, 200.0, 100.0)
                .unwrap();
        assert!((center.0 - 960.0).abs() < 10.0);
        assert!((center.1 - 540.0).abs() < 10.0);
    }

    #[test]
    fn test_selection_anchor_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let code = "fn render_frame() {\n/* ANCHOR_START: intro.title */\n\"fframes desktop studio\"\n/* ANCHOR_END: intro.title */\n}\n";
        fs::write(src_dir.join("main.rs"), code).unwrap();

        let spike =
            SelectionSpike::new_title_fixture(tmp.path(), "src/main.rs", 1, "rev1").unwrap();

        // Click on title bounds
        let info = spike
            .select_at_point(tmp.path(), 150.0, 250.0, 1, "rev1")
            .expect("selection succeeds");
        assert_eq!(info.element_id, "intro.title");
        assert_eq!(info.code_snippet, "\"fframes desktop studio\"");
        let mut invalid = spike.clone();
        invalid.source_path = "../outside.rs".into();
        assert!(matches!(
            invalid.select_at_point(tmp.path(), 150., 250., 1, "rev1"),
            Err(SelectionError::PathEscapesRoot(_))
        ));
        invalid = spike.clone();
        invalid.byte_start = 0;
        assert!(matches!(
            invalid.select_at_point(tmp.path(), 150., 250., 1, "rev1"),
            Err(SelectionError::MarkerMismatch(_))
        ));
        invalid = spike.clone();
        invalid.containing_symbol = "missing".into();
        assert!(matches!(
            invalid.select_at_point(tmp.path(), 150., 250., 1, "rev1"),
            Err(SelectionError::MarkerMismatch(_))
        ));

        // Click outside bounds
        let err_outside = spike
            .select_at_point(tmp.path(), 10.0, 10.0, 1, "rev1")
            .unwrap_err();
        assert!(matches!(
            err_outside,
            SelectionError::NoElementAtPoint { .. }
        ));

        // Stale generation check
        let err_stale = spike
            .select_at_point(tmp.path(), 150.0, 250.0, 2, "rev1")
            .unwrap_err();
        assert!(matches!(err_stale, SelectionError::StaleGeneration { .. }));

        // Stale revision check
        let err_rev = spike
            .select_at_point(tmp.path(), 150.0, 250.0, 1, "rev2")
            .unwrap_err();
        assert!(matches!(err_rev, SelectionError::StaleRevision { .. }));

        // File modified on disk check
        fs::write(src_dir.join("main.rs"), "modified code here").unwrap();
        let err_modified = spike
            .select_at_point(tmp.path(), 150.0, 250.0, 1, "rev1")
            .unwrap_err();
        assert!(matches!(
            err_modified,
            SelectionError::SourceModified { .. }
        ));
    }
}

/// Frame metadata is retained alongside the image, never fetched for a later playhead.
#[derive(Clone)]
pub struct DisplayedSourceFrame {
    pub header: FrameHeader,
    pub elements: Vec<ElementMetadata>,
    pub project_root: std::path::PathBuf,
}

impl DisplayedSourceFrame {
    pub fn select(
        &self,
        x: f32,
        y: f32,
        generation: u64,
        revision: &str,
    ) -> Result<SelectedElementInfo, SelectionError> {
        if self.header.source_revision != revision {
            return Err(SelectionError::StaleRevision {
                expected: revision.into(),
                actual: self.header.source_revision.clone(),
            });
        }
        let mut identities = std::collections::HashSet::new();
        for element in &self.elements {
            if !identities.insert((
                &element.scene_instance_id,
                &element.element_id,
                &element.instance_key,
            )) {
                return Err(SelectionError::MarkerMismatch(
                    "duplicate element identity".into(),
                ));
            }
        }
        let element = self
            .elements
            .iter()
            .filter(|e| {
                let b = &e.bounds;
                [b.x, b.y, b.width, b.height].iter().all(|v| v.is_finite())
                    && b.width > 0.
                    && b.height > 0.
                    && x >= b.x
                    && x <= b.x + b.width
                    && y >= b.y
                    && y <= b.y + b.height
            })
            .max_by_key(|e| e.paint_order)
            .ok_or(SelectionError::NoElementAtPoint { x, y })?;
        SelectionSpike::from_metadata(element, self.header.worker_generation).select_at_point(
            &self.project_root,
            x,
            y,
            generation,
            revision,
        )
    }
}
