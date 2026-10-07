//! Opt-in semantic object keys and geometry from the converted render tree.
use std::{
    collections::{BTreeSet, HashSet},
    hash::{Hash, Hasher},
};

use usvgr::{Node, Tree};

pub const MAX_EDITOR_KEY_COMPONENT_BYTES: usize = 256;
pub const MAX_EDITOR_OBJECTS_PER_FRAME: usize = 4096;
const MAX_EDITOR_TREE_DEPTH: usize = 128;
const RENDER_ID_PREFIX: &str = "fframes.editor.v1.";

/// Stable author-supplied identity for one rendered object occurrence.
///
/// All four strings are semantic keys. None are derived from Rust type names, source
/// text, raster cache hashes, tree position or paint order.
#[derive(Debug, Clone)]
pub struct EditorObjectKey {
    pub scene_instance_key: String,
    pub component_key: String,
    pub object_key: String,
    pub repeat_key: String,
    pub source_anchor: Option<EditorSourceAnchor>,
    pub style_tokens: Vec<String>,
}

/// Explicit, project-relative source evidence associated with an editor object.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EditorSourceAnchor {
    pub path: String,
    pub symbol: String,
    pub marker: Option<String>,
}

impl PartialEq for EditorObjectKey {
    fn eq(&self, other: &Self) -> bool {
        self.scene_instance_key == other.scene_instance_key
            && self.component_key == other.component_key
            && self.object_key == other.object_key
            && self.repeat_key == other.repeat_key
    }
}

impl Eq for EditorObjectKey {}

impl Hash for EditorObjectKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scene_instance_key.hash(state);
        self.component_key.hash(state);
        self.object_key.hash(state);
        self.repeat_key.hash(state);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorMetadataError {
    InvalidKey,
    InvalidSvgDocument,
    DuplicateKey,
    LimitExceeded,
}

impl std::fmt::Display for EditorMetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => {
                "editor identity fields must be nonempty, printable, and at most 256 UTF-8 bytes"
            }
            Self::InvalidSvgDocument => "editor object requires a valid SVG document or subtree",
            Self::DuplicateKey => "duplicate editor object identity in one rendered frame",
            Self::LimitExceeded => "editor metadata exceeds its object-count or tree-depth limit",
        })
    }
}

impl std::error::Error for EditorMetadataError {}

impl EditorObjectKey {
    pub fn new(
        scene_instance_key: impl Into<String>,
        component_key: impl Into<String>,
        object_key: impl Into<String>,
        repeat_key: impl Into<String>,
    ) -> Result<Self, EditorMetadataError> {
        let key = Self {
            scene_instance_key: scene_instance_key.into(),
            component_key: component_key.into(),
            object_key: object_key.into(),
            repeat_key: repeat_key.into(),
            source_anchor: None,
            style_tokens: Vec::new(),
        };
        if [
            &key.scene_instance_key,
            &key.component_key,
            &key.object_key,
            &key.repeat_key,
        ]
        .iter()
        .any(|part| {
            part.is_empty()
                || part.len() > MAX_EDITOR_KEY_COMPONENT_BYTES
                || part.chars().any(char::is_control)
        }) {
            return Err(EditorMetadataError::InvalidKey);
        }
        Ok(key)
    }

    pub fn with_source_anchor(
        mut self,
        path: impl Into<String>,
        symbol: impl Into<String>,
        marker: Option<&str>,
    ) -> Result<Self, EditorMetadataError> {
        let path = path.into();
        let symbol = symbol.into();
        let marker = marker.map(str::to_owned);
        let safe_path = !path.is_empty()
            && path.len() <= 512
            && !path.starts_with('/')
            && !path.chars().any(|ch| matches!(ch, '\\' | ':' | '\0'))
            && !path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..");
        if !safe_path
            || symbol.is_empty()
            || symbol.len() > 512
            || symbol.chars().any(char::is_control)
            || marker.as_ref().is_some_and(|marker| {
                marker.is_empty() || marker.len() > 256 || marker.chars().any(char::is_control)
            })
        {
            return Err(EditorMetadataError::InvalidKey);
        }
        self.source_anchor = Some(EditorSourceAnchor {
            path,
            symbol,
            marker,
        });
        Ok(self)
    }

    /// Register typed style tokens used by this object; this is explicit provenance,
    /// not an inference from nearby literals or arbitrary source text.
    pub fn with_style_tokens(
        mut self,
        tokens: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, EditorMetadataError> {
        let tokens: BTreeSet<String> = tokens.into_iter().map(Into::into).collect();
        if tokens.len() > 64
            || tokens.iter().any(|token| {
                token.is_empty() || token.len() > 128 || token.chars().any(char::is_control)
            })
        {
            return Err(EditorMetadataError::InvalidKey);
        }
        self.style_tokens = tokens.into_iter().collect();
        Ok(self)
    }

    /// A reversible, delimiter-safe SVG ID carrying the semantic key tuple.
    pub fn render_id(&self) -> String {
        let mut encoded = String::from(RENDER_ID_PREFIX);
        let key_parts = [
            &self.scene_instance_key,
            &self.component_key,
            &self.object_key,
            &self.repeat_key,
        ];
        for (index, part) in key_parts.into_iter().enumerate() {
            if index != 0 {
                encoded.push('.');
            }
            encode_field(&mut encoded, part.as_bytes());
        }
        if let Some(anchor) = &self.source_anchor {
            encoded.push('.');
            encode_field(&mut encoded, anchor.path.as_bytes());
            encoded.push('.');
            encode_field(&mut encoded, anchor.symbol.as_bytes());
            encoded.push('.');
            match &anchor.marker {
                Some(marker) => encode_field(&mut encoded, marker.as_bytes()),
                None => encoded.push('-'),
            }
        }
        if !self.style_tokens.is_empty() {
            encoded.push('.');
            encode_field(&mut encoded, self.style_tokens.join("\0").as_bytes());
        }
        encoded
    }

    pub fn from_render_id(id: &str) -> Option<Self> {
        let fields: Vec<_> = id.strip_prefix(RENDER_ID_PREFIX)?.split('.').collect();
        let field_count = fields.len();
        let mut decoded = Vec::with_capacity(7);
        for field in fields {
            if field == "-" {
                decoded.push(String::new());
                continue;
            }
            if field.is_empty() || field.len() % 2 != 0 {
                return None;
            }
            let bytes = field
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|hex| {
                    let high = (hex[0] as char).to_digit(16)?;
                    let low = (hex[1] as char).to_digit(16)?;
                    Some((high * 16 + low) as u8)
                })
                .collect::<Option<Vec<_>>>()?;
            decoded.push(String::from_utf8(bytes).ok()?);
        }
        if decoded.len() != 4 && decoded.len() != 5 && decoded.len() != 7 && decoded.len() != 8 {
            return None;
        }
        let mut decoded = decoded.into_iter();
        let mut key = Self::new(
            decoded.next()?,
            decoded.next()?,
            decoded.next()?,
            decoded.next()?,
        )
        .ok()?;
        if field_count == 5 {
            let tokens = decoded.next()?;
            if tokens.is_empty() {
                return None;
            }
            key = key
                .with_style_tokens(tokens.split('\0').map(str::to_owned))
                .ok()?;
        } else if field_count >= 7 {
            let path = decoded.next()?;
            let symbol = decoded.next()?;
            let marker = decoded.next()?;
            key = key
                .with_source_anchor(
                    path,
                    symbol,
                    if marker.is_empty() {
                        None
                    } else {
                        Some(&marker)
                    },
                )
                .ok()?;
            if field_count == 8 {
                let tokens = decoded.next()?;
                let tokens = if tokens.is_empty() {
                    Vec::new()
                } else {
                    tokens.split('\0').map(str::to_owned).collect()
                };
                key = key.with_style_tokens(tokens).ok()?;
            }
        }
        Some(key)
    }
}

fn encode_field(encoded: &mut String, bytes: &[u8]) {
    use std::fmt::Write as _;
    for byte in bytes {
        write!(encoded, "{byte:02x}").expect("writing to String is infallible");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorGeometrySupport {
    ExactBounds,
    ApproximateBounds,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditorObjectGeometry {
    pub key: EditorObjectKey,
    pub style_tokens: Vec<String>,
    pub parent: Option<EditorObjectKey>,
    pub source_anchor: Option<EditorSourceAnchor>,
    /// Full-resolution video pixel coordinates, independent of preview raster scale.
    pub bounds: EditorRect,
    pub paint_order: u32,
    pub support: EditorGeometrySupport,
}

/// One revision-independent frame's geometry. The worker binds it to preview identity,
/// frame index and seek serial before publishing it alongside that frame's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorFrameGeometry {
    pub video_width: u32,
    pub video_height: u32,
    pub objects: Vec<EditorObjectGeometry>,
}

fn tree_rect(rect: usvgr::Rect, tree: &Tree, width: u32, height: u32) -> EditorRect {
    let size = tree.size();
    let sx = width as f32 / size.width();
    let sy = height as f32 / size.height();
    let x = (rect.x() * sx).clamp(0.0, width as f32);
    let y = (rect.y() * sy).clamp(0.0, height as f32);
    let right = ((rect.x() + rect.width()) * sx).clamp(0.0, width as f32);
    let bottom = ((rect.y() + rect.height()) * sy).clamp(0.0, height as f32);
    EditorRect {
        x,
        y,
        width: (right - x).max(0.0),
        height: (bottom - y).max(0.0),
    }
}

struct EditorGeometryWalker<'a> {
    tree: &'a Tree,
    video_width: u32,
    video_height: u32,
    next_order: u32,
    seen: HashSet<EditorObjectKey>,
    objects: Vec<EditorObjectGeometry>,
}

impl EditorGeometryWalker<'_> {
    fn walk(
        &mut self,
        group: &usvgr::Group,
        depth: usize,
        parent: Option<&EditorObjectKey>,
        inherited_approximation: bool,
    ) -> Result<(), EditorMetadataError> {
        if depth > MAX_EDITOR_TREE_DEPTH {
            return Err(EditorMetadataError::LimitExceeded);
        }
        for node in group.children() {
            let order = self.next_order;
            self.next_order = self
                .next_order
                .checked_add(1)
                .ok_or(EditorMetadataError::LimitExceeded)?;
            let is_editor_group = matches!(node, Node::Group(_));
            let key = if is_editor_group {
                EditorObjectKey::from_render_id(node.id())
            } else {
                None
            };
            let mut child_parent = parent;
            let mut child_approximation = inherited_approximation;
            if let (Some(key), Node::Group(object_group)) = (&key, node) {
                if !self.seen.insert(key.clone()) {
                    return Err(EditorMetadataError::DuplicateKey);
                }
                if self.objects.len() >= MAX_EDITOR_OBJECTS_PER_FRAME {
                    return Err(EditorMetadataError::LimitExceeded);
                }
                let effects = object_group.clip_path().is_some()
                    || object_group.mask().is_some()
                    || !object_group.filters().is_empty();
                let opacity = object_group.opacity().get();
                let rect = object_group.abs_stroke_bounding_box();
                let visible_bounds = rect.width() > 0.0
                    && rect.height() > 0.0
                    && opacity.is_finite()
                    && opacity > 0.0;
                self.objects.push(EditorObjectGeometry {
                    key: key.clone(),
                    style_tokens: key.style_tokens.clone(),
                    parent: parent.cloned(),
                    source_anchor: key.source_anchor.clone(),
                    bounds: tree_rect(rect, self.tree, self.video_width, self.video_height),
                    paint_order: order,
                    support: if !visible_bounds {
                        EditorGeometrySupport::Unsupported
                    } else if inherited_approximation || effects || opacity < 1.0 {
                        EditorGeometrySupport::ApproximateBounds
                    } else {
                        EditorGeometrySupport::ExactBounds
                    },
                });
                child_parent = Some(key);
                child_approximation |= effects || opacity < 1.0 || opacity == 0.0;
            }
            if let Node::Group(child) = node {
                self.walk(child, depth + 1, child_parent, child_approximation)?;
            }
        }
        Ok(())
    }
}

/// Collects opt-in editor IDs from the same converted tree used by the frame renderer.
pub fn editor_geometry(
    tree: &Tree,
    video_width: u32,
    video_height: u32,
) -> Result<EditorFrameGeometry, EditorMetadataError> {
    if video_width == 0 || video_height == 0 {
        return Err(EditorMetadataError::LimitExceeded);
    }
    let mut walker = EditorGeometryWalker {
        tree,
        video_width,
        video_height,
        next_order: 0,
        seen: HashSet::new(),
        objects: Vec::new(),
    };
    walker.walk(tree.root(), 0, None, false)?;
    Ok(EditorFrameGeometry {
        video_width,
        video_height,
        objects: walker.objects,
    })
}
