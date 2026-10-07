use crate::error::{FFramesError, Result};
#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
use std::fmt::Write as _;
use std::{fmt, iter::FromIterator};

#[derive(Default, Clone, Debug)]
pub struct Svgr<'a> {
    #[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
    pub value: String,
    #[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
    pub marker: std::marker::PhantomData<&'a str>,
    #[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
    pub svg_tree: usvgr::svgtree::NestedSvgDocument<'a, usvgr::svgtree::NestedNodeData<'a>>,
}

impl<'a> Svgr<'a> {
    /// Wrap this SVG subtree in an explicit, stable editor object identity.
    ///
    /// The wrapper is ordinary SVG and therefore renders identically. Its encoded
    /// group ID survives both the compile-time nested-tree and runtime-string paths.
    pub fn with_editor_object(self, key: &crate::EditorObjectKey) -> Self {
        let render_id = key.render_id();
        #[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
        {
            use usvgr::svgtree::{
                AId, Attribute, EId, NestedNodeData, NestedNodeKind, SvgAttributeValue,
            };

            let group = |children| NestedNodeData {
                kind: NestedNodeKind::Element { tag_name: EId::G },
                attrs: vec![Attribute {
                    name: AId::Id,
                    value: SvgAttributeValue::from(render_id.clone()),
                }]
                .into_boxed_slice(),
                children,
                static_hash: None,
            };
            let mut document = self.svg_tree;
            if let [Some(root)] = document.nodes.as_mut_slice()
                && root.kind == (NestedNodeKind::Element { tag_name: EId::Svg })
            {
                let children = std::mem::take(&mut root.children);
                root.children = vec![Some(group(children))];
                return Self { svg_tree: document };
            }
            Self {
                svg_tree: usvgr::svgtree::NestedSvgDocument::from_nodes(vec![Some(group(
                    document.nodes,
                ))]),
            }
        }
        #[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
        {
            let wrapped = if let Ok((root_start, root_end)) = svg_document_bounds(&self.value) {
                let mut wrapped = String::with_capacity(self.value.len() + render_id.len() + 16);
                wrapped.push_str(&self.value[..root_start]);
                let _ = write!(wrapped, "<g id=\"{render_id}\">");
                wrapped.push_str(&self.value[root_start..root_end]);
                wrapped.push_str("</g>");
                wrapped.push_str(&self.value[root_end..]);
                wrapped
            } else {
                let mut wrapped = String::with_capacity(self.value.len() + render_id.len() + 64);
                wrapped.push_str("<svg xmlns=\"http://www.w3.org/2000/svg\">");
                let _ = write!(wrapped, "<g id=\"{render_id}\">");
                wrapped.push_str(&self.value);
                wrapped.push_str("</g>");
                wrapped.push_str("</svg>");
                wrapped
            };
            Self {
                value: wrapped,
                marker: std::marker::PhantomData,
            }
        }
    }

    #[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
    pub fn into_svg_tree(
        self,
        opt: &usvgr::Options,
        cache: &mut usvgr::Cache,
        fontdb: &usvgr::fontdb::Database,
    ) -> Result<usvgr::Tree> {
        usvgr::Tree::from_nested_svgtree_with_cache(&self.svg_tree, opt, cache, fontdb)
            .map_err(FFramesError::ParserError)
    }

    #[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
    pub fn into_svg_tree(
        self,
        opt: &usvgr::Options,
        _cache: &mut usvgr::Cache,
        fontdb: &usvgr::fontdb::Database,
    ) -> Result<usvgr::Tree> {
        usvgr::Tree::from_str(&self.value, opt, fontdb).map_err(FFramesError::ParserError)
    }

    #[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
    pub fn as_subtree(self) -> Vec<Option<usvgr::svgtree::NestedNodeData<'a>>> {
        self.svg_tree.nodes
    }

    #[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
    pub fn as_subtree(self) -> Vec<Option<usvgr::svgtree::NestedNodeData<'a>>> {
        unimplemented!(
            "Subtrees are not available when using runtime svg tree, if you see this message it means that feature flags are set incorrectly."
        )
    }

    pub fn empty() -> Self {
        Self::default()
    }
}

#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
fn svg_document_bounds(
    svg: &str,
) -> std::result::Result<(usize, usize), crate::EditorMetadataError> {
    let document = usvgr::roxmltree::Document::parse(svg)
        .map_err(|_| crate::EditorMetadataError::InvalidSvgDocument)?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(crate::EditorMetadataError::InvalidSvgDocument);
    }
    let range = root.range();
    let bytes = svg.as_bytes();
    let mut quote = None;
    let opening_end = (range.start..range.end)
        .find(|index| {
            let byte = bytes[*index];
            match (quote, byte) {
                (Some(expected), value) if expected == value => quote = None,
                (None, b'\'' | b'"') => quote = Some(byte),
                (None, b'>') => return true,
                _ => {}
            }
            false
        })
        .ok_or(crate::EditorMetadataError::InvalidSvgDocument)?
        + 1;
    let closing_start = svg[opening_end..range.end]
        .rfind("</")
        .map(|offset| opening_end + offset)
        .ok_or(crate::EditorMetadataError::InvalidSvgDocument)?;
    Ok((opening_end, closing_start))
}

#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
impl From<String> for Svgr<'_> {
    fn from(value: String) -> Self {
        Svgr {
            value,
            marker: std::marker::PhantomData,
        }
    }
}

#[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
impl From<String> for Svgr<'_> {
    fn from(val: String) -> Self {
        use usvgr::svgtree::{NestedNodeData, NestedSvgDocument};

        Svgr {
            svg_tree: NestedSvgDocument::from_nodes(vec![
                Some(NestedNodeData {
                    kind: usvgr::svgtree::NestedNodeKind::Text(
                        usvgr::svgtree::roxmltree::StringStorage::new_owned(val)
                    ),
                    attrs: Box::new([]),
                    children: vec![],
                    static_hash: None,
                });
                1
            ]),
        }
    }
}

#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
impl<'a> FromIterator<Svgr<'a>> for Svgr<'a> {
    fn from_iter<T: IntoIterator<Item = Svgr<'a>>>(iter: T) -> Self {
        Svgr {
            marker: std::marker::PhantomData,
            value: iter
                .into_iter()
                .fold(String::new(), |acc, s| acc + &s.value),
        }
    }
}

#[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
impl<'a> FromIterator<Svgr<'a>> for Svgr<'a> {
    fn from_iter<T: IntoIterator<Item = Svgr<'a>>>(iter: T) -> Self {
        let mut child_nodes = usvgr::svgtree::NestedSvgDocument::from_nodes(vec![]);

        for sub_tree in iter {
            let mut nested_tree = sub_tree.svg_tree;

            child_nodes.nodes.append(&mut nested_tree.nodes);
        }

        Svgr {
            svg_tree: child_nodes,
        }
    }
}

#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
impl<'a> From<&'a str> for Svgr<'a> {
    fn from(val: &'a str) -> Self {
        Svgr {
            value: val.to_string(),
            marker: std::marker::PhantomData,
        }
    }
}

#[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
impl<'a> From<&'a str> for Svgr<'a> {
    fn from(val: &'a str) -> Self {
        use usvgr::svgtree::{NestedNodeData, NestedSvgDocument};

        Svgr {
            svg_tree: NestedSvgDocument::from_nodes(vec![
                Some(NestedNodeData {
                    kind: usvgr::svgtree::NestedNodeKind::Text(
                        usvgr::svgtree::roxmltree::StringStorage::Borrowed(val)
                    ),
                    attrs: Box::new([]),
                    children: vec![],
                    static_hash: None,
                });
                1
            ]),
        }
    }
}

impl<'a> From<Vec<Svgr<'a>>> for Svgr<'a> {
    fn from(val: Vec<Svgr<'a>>) -> Svgr<'a> {
        FromIterator::from_iter(val)
    }
}

#[cfg(all(feature = "compile-time-svgtree", not(target_arch = "wasm32")))]
impl fmt::Display for Svgr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.svg_tree)
    }
}

#[cfg(any(not(feature = "compile-time-svgtree"), target_arch = "wasm32"))]
impl fmt::Display for Svgr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}
