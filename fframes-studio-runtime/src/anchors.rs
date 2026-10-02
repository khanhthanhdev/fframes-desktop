use fframes_studio_protocol::{ElementMetadata, Rect};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ElementRegistration {
    pub source_revision: String,
    pub scene_instance_id: String,
    pub element_id: String,
    pub instance_key: String,
    pub bounds: Rect,
    pub paint_order: u32,
    pub source_path: String,
    pub containing_symbol: String,
    pub source_hash: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

impl ElementRegistration {
    pub fn to_protocol_metadata(&self) -> ElementMetadata {
        ElementMetadata {
            source_revision: self.source_revision.clone(),
            scene_instance_id: self.scene_instance_id.clone(),
            element_id: self.element_id.clone(),
            instance_key: self.instance_key.clone(),
            bounds: self.bounds.clone(),
            paint_order: self.paint_order,
            source_path: self.source_path.clone(),
            containing_symbol: self.containing_symbol.clone(),
            source_hash: self.source_hash.clone(),
            byte_start: self.byte_start,
            byte_end: self.byte_end,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnchorError {
    #[error("stale source revision: expected '{expected}', actual '{actual}'")]
    StaleRevision { expected: String, actual: String },
    #[error("stale worker generation: expected {expected}, actual {actual}")]
    StaleGeneration { expected: u64, actual: u64 },
    #[error("path traversal detected: '{0}' escapes project root")]
    PathEscapesRoot(String),
    #[error("source file not found: {0}")]
    FileNotFound(String),
    #[error("source hash mismatch: expected {expected}, actual {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("byte span out of bounds: {start}..{end} in file of length {len}")]
    SpanOutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("byte span does not align to valid UTF-8 character boundaries: {start}..{end}")]
    InvalidUtf8Boundary { start: usize, end: usize },
    #[error("anchor markers missing or ambiguous for element '{0}'")]
    MarkerMismatch(String),
    #[error("io error: {0}")]
    Io(String),
}

/// Validates an element's source anchor against the actual file on disk.
/// Ensures the file exists within `workspace_root`, the hash matches,
/// byte bounds are valid UTF-8 boundaries, and returns the resolved code snippet.
pub fn validate_source_anchor(
    workspace_root: &Path,
    registration: &ElementRegistration,
    current_revision: &str,
    current_generation: u64,
    anchor_generation: u64,
) -> Result<String, AnchorError> {
    if anchor_generation != current_generation {
        return Err(AnchorError::StaleGeneration {
            expected: current_generation,
            actual: anchor_generation,
        });
    }

    if registration.source_revision != current_revision {
        return Err(AnchorError::StaleRevision {
            expected: current_revision.to_string(),
            actual: registration.source_revision.clone(),
        });
    }
    // Path traversal check
    let clean_path = Path::new(&registration.source_path);
    if clean_path.is_absolute()
        || registration.source_path.contains("..")
        || registration.source_path.starts_with('/')
    {
        return Err(AnchorError::PathEscapesRoot(
            registration.source_path.clone(),
        ));
    }

    let full_path = workspace_root.join(clean_path);
    if !full_path.exists() {
        return Err(AnchorError::FileNotFound(registration.source_path.clone()));
    }

    let canonical_root = workspace_root
        .canonicalize()
        .map_err(|e| AnchorError::Io(e.to_string()))?;
    let canonical_file = full_path
        .canonicalize()
        .map_err(|e| AnchorError::Io(e.to_string()))?;

    if !canonical_file.starts_with(&canonical_root) {
        return Err(AnchorError::PathEscapesRoot(
            registration.source_path.clone(),
        ));
    }

    let content =
        fs::read_to_string(&canonical_file).map_err(|e| AnchorError::Io(e.to_string()))?;

    // Verify whole-file SHA-256
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let actual_hash = format!("{:x}", hasher.finalize());

    if !actual_hash.eq_ignore_ascii_case(&registration.source_hash) {
        return Err(AnchorError::HashMismatch {
            expected: registration.source_hash.clone(),
            actual: actual_hash,
        });
    }

    // Verify byte span boundaries
    if registration.byte_start > registration.byte_end || registration.byte_end > content.len() {
        return Err(AnchorError::SpanOutOfBounds {
            start: registration.byte_start,
            end: registration.byte_end,
            len: content.len(),
        });
    }

    if !content.is_char_boundary(registration.byte_start)
        || !content.is_char_boundary(registration.byte_end)
    {
        return Err(AnchorError::InvalidUtf8Boundary {
            start: registration.byte_start,
            end: registration.byte_end,
        });
    }

    let snippet = &content[registration.byte_start..registration.byte_end];

    // Verify marker uniqueness if markers are used
    let start_marker = format!("/* ANCHOR_START: {} */", registration.element_id);
    let end_marker = format!("/* ANCHOR_END: {} */", registration.element_id);
    if content.contains(&start_marker) {
        let count_start = content.matches(&start_marker).count();
        let count_end = content.matches(&end_marker).count();
        if count_start != 1 || count_end != 1 {
            return Err(AnchorError::MarkerMismatch(registration.element_id.clone()));
        }
    }

    Ok(snippet.to_string())
}

/// Performs hit-testing across registered elements.
/// Chooses the topmost element by `paint_order` whose bounding box contains `(x, y)`.
pub fn hit_test_elements<'a>(
    registrations: &'a [ElementRegistration],
    x: f32,
    y: f32,
) -> Option<&'a ElementRegistration> {
    let mut matching: Vec<&'a ElementRegistration> = registrations
        .iter()
        .filter(|reg| {
            x >= reg.bounds.x
                && x <= reg.bounds.x + reg.bounds.width
                && y >= reg.bounds.y
                && y <= reg.bounds.y + reg.bounds.height
        })
        .collect();

    // Sort descending by paint_order (topmost first)
    matching.sort_by_key(|a| std::cmp::Reverse(a.paint_order));
    matching.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hit_testing_paint_order() {
        let bottom = ElementRegistration {
            source_revision: "rev1".into(),
            scene_instance_id: "scene1".into(),
            element_id: "bg_rect".into(),
            instance_key: "k1".into(),
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            paint_order: 1,
            source_path: "src/main.rs".into(),
            containing_symbol: "render_frame".into(),
            source_hash: "".into(),
            byte_start: 0,
            byte_end: 10,
        };

        let top = ElementRegistration {
            source_revision: "rev1".into(),
            scene_instance_id: "scene1".into(),
            element_id: "title_text".into(),
            instance_key: "k2".into(),
            bounds: Rect {
                x: 20.0,
                y: 20.0,
                width: 50.0,
                height: 20.0,
            },
            paint_order: 2,
            source_path: "src/main.rs".into(),
            containing_symbol: "render_frame".into(),
            source_hash: "".into(),
            byte_start: 10,
            byte_end: 20,
        };

        let list = vec![bottom.clone(), top.clone()];

        // Click outside both
        assert!(hit_test_elements(&list, 200.0, 200.0).is_none());

        // Click on background only
        let hit_bg = hit_test_elements(&list, 5.0, 5.0).expect("hits background");
        assert_eq!(hit_bg.element_id, "bg_rect");

        // Click on overlap (both match) -> top wins
        let hit_top = hit_test_elements(&list, 30.0, 25.0).expect("hits top title");
        assert_eq!(hit_top.element_id, "title_text");
    }

    #[test]
    fn test_validate_source_anchor_success_and_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("src");
        fs::create_dir_all(&file_path).unwrap();

        let code = "fn render() {\n    /* ANCHOR_START: title */\n    let title = \"fframes\";\n    /* ANCHOR_END: title */\n}\n";
        fs::write(file_path.join("main.rs"), code).unwrap();

        let mut hasher = Sha256::new();
        hasher.update(code.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let start_byte = code.find("let title").unwrap();
        let end_byte = start_byte + "let title = \"fframes\";".len();

        let reg = ElementRegistration {
            source_revision: "rev1".into(),
            scene_instance_id: "intro".into(),
            element_id: "title".into(),
            instance_key: "k1".into(),
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            paint_order: 1,
            source_path: "src/main.rs".into(),
            containing_symbol: "render".into(),
            source_hash: hash.clone(),
            byte_start: start_byte,
            byte_end: end_byte,
        };
        // 1. Success case
        let snippet = validate_source_anchor(tmp.path(), &reg, "rev1", 1, 1).expect("valid anchor");
        assert_eq!(snippet, "let title = \"fframes\";");

        // 2. Stale generation
        let err_gen = validate_source_anchor(tmp.path(), &reg, "rev1", 2, 1).unwrap_err();
        assert!(matches!(err_gen, AnchorError::StaleGeneration { .. }));

        // 2b. Stale revision
        let err_rev = validate_source_anchor(tmp.path(), &reg, "rev2", 1, 1).unwrap_err();
        assert!(matches!(err_rev, AnchorError::StaleRevision { .. }));

        // 3. Path traversal rejection
        let mut reg_escape = reg.clone();
        reg_escape.source_path = "../escaping.rs".into();
        let err_esc = validate_source_anchor(tmp.path(), &reg_escape, "rev1", 1, 1).unwrap_err();
        assert!(matches!(err_esc, AnchorError::PathEscapesRoot(_)));

        // 4. File modification (hash mismatch)
        let mut reg_mod = reg.clone();
        reg_mod.source_hash =
            "0000000000000000000000000000000000000000000000000000000000000000".into();
        let err_hash = validate_source_anchor(tmp.path(), &reg_mod, "rev1", 1, 1).unwrap_err();
        assert!(matches!(err_hash, AnchorError::HashMismatch { .. }));
    }
}
