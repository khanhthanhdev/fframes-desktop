//! Deterministic, syntax-only Rust source indexing over caller-supplied immutable bytes.
//!
//! This module deliberately does not open paths or run rustc/macros. Callers must obtain
//! bytes from a verified `SourceInventory` and the checkpoint object store, then pass the
//! expected per-file digest here.
use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::LineColumn;
use serde::Serialize;
use sha2::{Digest, Sha256};
use syn::{spanned::Spanned, visit::Visit};

use crate::{ProjectPath, SourceRevision, revision::FileKind};

pub const SOURCE_INDEX_SCHEMA_VERSION: u32 = 1;
pub const MAX_INDEX_RUST_FILES: usize = 256;
pub const MAX_INDEX_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_INDEX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_LOOKUP_SNIPPETS: usize = 8;
pub const MAX_LOOKUP_SNIPPET_BYTES: usize = 4 * 1024;
pub const MAX_LOOKUP_TOTAL_BYTES: usize = 32 * 1024;
pub const MAX_HELPER_DEPTH: usize = 2;
pub const MAX_HELPER_EDGES: usize = 32;

#[derive(Debug, Clone)]
pub struct SourceIndexInput {
    pub path: ProjectPath,
    pub kind: FileKind,
    pub expected_sha256: String,
    pub expected_size: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceIndexError {
    InvalidDigest(String),
    HashMismatch(String),
    SizeMismatch(String),
    Cancelled,
    InvalidAnchor(String),
}

impl std::fmt::Display for SourceIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDigest(path) => write!(f, "invalid expected SHA-256 for {path}"),
            Self::HashMismatch(path) => write!(f, "immutable source hash mismatch for {path}"),
            Self::SizeMismatch(path) => {
                write!(f, "immutable source size limit exceeded for {path}")
            }
            Self::Cancelled => f.write_str("source indexing cancelled"),
            Self::InvalidAnchor(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for SourceIndexError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RustSymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Type,
    Constant,
    Static,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: RustSymbolKind,
    pub span: ByteSpan,
    pub parent: Option<String>,
    /// Syntactically visible call/path names; this is not type or macro resolution.
    pub references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileDiagnostic {
    pub path: ProjectPath,
    pub message: String,
}

#[derive(Debug, Clone)]
struct IndexedFile {
    path: ProjectPath,
    sha256: String,
    text: String,
    symbols: Vec<RustSymbol>,
}

#[derive(Debug, Clone)]
pub struct SourceIndex {
    pub revision: SourceRevision,
    pub schema_version: u32,
    files: BTreeMap<ProjectPath, IndexedFile>,
    pub diagnostics: Vec<SourceFileDiagnostic>,
    pub indexed_files: usize,
    pub indexed_bytes: usize,
    pub truncated_files: usize,
    pub truncated_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAnchor {
    pub path: ProjectPath,
    pub symbol: String,
    pub expected_sha256: String,
    /// Optional unique literal marker constrained to the selected Rust symbol.
    pub marker: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSnippet {
    pub path: ProjectPath,
    pub symbol: String,
    pub sha256: String,
    /// Exact source range associated with this snippet (the marker or symbol).
    pub anchor_span: ByteSpan,
    /// Returned UTF-8-aligned byte window, which may be truncated.
    pub span: ByteSpan,
    pub confidence: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLookup {
    pub revision: String,
    pub snippets: Vec<SourceSnippet>,
    pub helper_candidates: Vec<String>,
    pub ambiguous: bool,
    pub truncated: bool,
    pub diagnostics: Vec<String>,
}

impl SourceIndex {
    /// Build a stable index from already captured inventory bytes. Files are sorted before
    /// applying ceilings, and every included byte vector is hash-verified before parsing.
    pub fn build(
        revision: SourceRevision,
        mut inputs: Vec<SourceIndexInput>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, SourceIndexError> {
        inputs.sort_by(|a, b| a.path.cmp(&b.path));
        let mut files = BTreeMap::new();
        let mut diagnostics = Vec::new();
        let mut indexed_bytes = 0usize;
        let mut rust_files = 0usize;
        let mut truncated_files = 0usize;
        let mut truncated_bytes = 0usize;

        for input in inputs {
            if input.kind != FileKind::Rust {
                continue;
            }
            if cancelled() {
                return Err(SourceIndexError::Cancelled);
            }
            if !valid_digest(&input.expected_sha256) {
                return Err(SourceIndexError::InvalidDigest(
                    input.path.as_str().to_owned(),
                ));
            }
            if hex_digest(&input.bytes) != input.expected_sha256 {
                return Err(SourceIndexError::HashMismatch(
                    input.path.as_str().to_owned(),
                ));
            }
            if input.bytes.len() as u64 != input.expected_size {
                return Err(SourceIndexError::SizeMismatch(
                    input.path.as_str().to_owned(),
                ));
            }
            if input.bytes.len() > MAX_INDEX_FILE_BYTES {
                truncated_files += 1;
                truncated_bytes = truncated_bytes.saturating_add(input.bytes.len());
                continue;
            }
            if rust_files >= MAX_INDEX_RUST_FILES
                || indexed_bytes.saturating_add(input.bytes.len()) > MAX_INDEX_TOTAL_BYTES
            {
                truncated_files += 1;
                truncated_bytes = truncated_bytes.saturating_add(input.bytes.len());
                continue;
            }
            rust_files += 1;
            indexed_bytes += input.bytes.len();
            let text = match String::from_utf8(input.bytes) {
                Ok(text) => text,
                Err(_) => {
                    diagnostics.push(SourceFileDiagnostic {
                        path: input.path,
                        message: "source is not valid UTF-8; syntax index omitted this file".into(),
                    });
                    continue;
                }
            };
            match syn::parse_file(&text) {
                Ok(syntax) => {
                    let symbols = collect_symbols(&syntax, &text);
                    files.insert(
                        input.path.clone(),
                        IndexedFile {
                            path: input.path,
                            sha256: input.expected_sha256,
                            text,
                            symbols,
                        },
                    );
                }
                Err(error) => diagnostics.push(SourceFileDiagnostic {
                    path: input.path,
                    message: format!("Rust syntax unavailable: {error}"),
                }),
            }
        }
        diagnostics.sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
        Ok(Self {
            revision,
            schema_version: SOURCE_INDEX_SCHEMA_VERSION,
            indexed_files: rust_files,
            indexed_bytes,
            truncated_files,
            truncated_bytes,
            files,
            diagnostics,
        })
    }

    pub fn lookup(&self, anchor: &SourceAnchor) -> Result<SourceLookup, SourceIndexError> {
        if anchor.symbol.is_empty() || anchor.symbol.len() > 512 {
            return Err(SourceIndexError::InvalidAnchor(
                "source symbol must be between 1 and 512 bytes".into(),
            ));
        }
        let file = self
            .files
            .get(&anchor.path)
            .ok_or_else(|| SourceIndexError::InvalidAnchor("source file is not indexed".into()))?;
        if file.sha256 != anchor.expected_sha256 {
            return Err(SourceIndexError::HashMismatch(
                anchor.path.as_str().to_owned(),
            ));
        }
        let matches: Vec<_> = file
            .symbols
            .iter()
            .filter(|symbol| symbol.name == anchor.symbol || symbol.qualified_name == anchor.symbol)
            .collect();
        if matches.len() != 1 {
            return Err(SourceIndexError::InvalidAnchor(if matches.is_empty() {
                format!(
                    "symbol {:?} was not found in {}",
                    anchor.symbol,
                    anchor.path.as_str()
                )
            } else {
                format!(
                    "symbol {:?} is ambiguous in {}",
                    anchor.symbol,
                    anchor.path.as_str()
                )
            }));
        }
        let symbol = matches[0];
        let (anchor_span, confidence) = if let Some(marker) = &anchor.marker {
            if marker.is_empty() || marker.len() > 256 || marker.chars().any(char::is_control) {
                return Err(SourceIndexError::InvalidAnchor(
                    "source marker is empty, oversized, or contains control characters".into(),
                ));
            }
            let Some(found) = unique_substring(&file.text, marker) else {
                return Err(SourceIndexError::InvalidAnchor(
                    "source marker must occur exactly once in the immutable file".into(),
                ));
            };
            if found.start < symbol.span.start || found.end > symbol.span.end {
                return Err(SourceIndexError::InvalidAnchor(
                    "source marker is not contained by the requested Rust symbol".into(),
                ));
            }
            (found, "explicit_marker")
        } else {
            (symbol.span, "syntax_symbol")
        };
        let (snippet_text, snippet_span, mut truncated) =
            bounded_text_around(&file.text, symbol.span, anchor_span);
        let mut snippets = vec![SourceSnippet {
            path: file.path.clone(),
            symbol: symbol.qualified_name.clone(),
            sha256: file.sha256.clone(),
            anchor_span,
            span: snippet_span,
            confidence,
            text: snippet_text,
        }];
        if let Some(parent) = symbol.parent.as_deref()
            && let Some(implementation) = file.symbols.iter().find(|candidate| {
                candidate.kind == RustSymbolKind::Impl && candidate.qualified_name == parent
            })
        {
            let (text, span, was_truncated) = bounded_text(&file.text, implementation.span);
            let used: usize = snippets.iter().map(|snippet| snippet.text.len()).sum();
            if snippets.len() < MAX_LOOKUP_SNIPPETS
                && used.saturating_add(text.len()) <= MAX_LOOKUP_TOTAL_BYTES
            {
                snippets.push(SourceSnippet {
                    path: file.path.clone(),
                    symbol: implementation.qualified_name.clone(),
                    sha256: file.sha256.clone(),
                    anchor_span: symbol.span,
                    span,
                    confidence: "containing_impl",
                    text,
                });
                truncated |= was_truncated;
            } else {
                truncated = true;
            }
        }
        let mut helper_candidates = BTreeSet::new();
        let mut queue = vec![(symbol, 0usize)];
        let mut visited = BTreeSet::from([(anchor.path.clone(), symbol.qualified_name.clone())]);
        let mut edges = 0usize;
        let mut ambiguous = false;
        while let Some((owner, depth)) = queue.pop() {
            if depth >= MAX_HELPER_DEPTH {
                continue;
            }
            for reference in &owner.references {
                if edges >= MAX_HELPER_EDGES {
                    break;
                }
                let name = reference.rsplit("::").next().unwrap_or(reference);
                if name.is_empty() || is_builtin_or_method(name) {
                    continue;
                }
                let candidates: Vec<_> = self
                    .files
                    .values()
                    .flat_map(|candidate_file| {
                        candidate_file
                            .symbols
                            .iter()
                            .map(move |candidate| (candidate_file, candidate))
                    })
                    .filter(|(_, candidate)| {
                        candidate.kind == RustSymbolKind::Function && candidate.name == name
                    })
                    .collect();
                if candidates.len() != 1 {
                    if !candidates.is_empty() {
                        ambiguous = true;
                        helper_candidates.insert(format!(
                            "{name} (ambiguous: {} candidates)",
                            candidates.len()
                        ));
                    }
                    continue;
                }
                let (helper_file, helper) = candidates[0];
                let key = (helper_file.path.clone(), helper.qualified_name.clone());
                if !visited.insert(key) {
                    continue;
                }
                edges += 1;
                helper_candidates.insert(helper.qualified_name.clone());
                if snippets.len() < MAX_LOOKUP_SNIPPETS {
                    let (text, helper_span, helper_truncated) =
                        bounded_text(&helper_file.text, helper.span);
                    let used: usize = snippets.iter().map(|snippet| snippet.text.len()).sum();
                    if used.saturating_add(text.len()) <= MAX_LOOKUP_TOTAL_BYTES {
                        snippets.push(SourceSnippet {
                            path: helper_file.path.clone(),
                            symbol: helper.qualified_name.clone(),
                            sha256: helper_file.sha256.clone(),
                            anchor_span: helper.span,
                            span: helper_span,
                            confidence: "syntactic_candidate",
                            text,
                        });
                        truncated |= helper_truncated;
                        queue.push((helper, depth + 1));
                    } else {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
        }
        let mut diagnostics: Vec<String> = self
            .diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.path.as_str(), diagnostic.message))
            .collect();
        if self.truncated_files > 0 {
            diagnostics.push(format!(
                "syntax index omitted {} Rust files ({} bytes); request explicit on-demand lookup",
                self.truncated_files, self.truncated_bytes
            ));
        }
        Ok(SourceLookup {
            revision: self.revision.as_str().to_owned(),
            snippets,
            helper_candidates: helper_candidates.into_iter().collect(),
            ambiguous,
            truncated: truncated || self.truncated_files > 0,
            diagnostics,
        })
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unique_substring(text: &str, marker: &str) -> Option<ByteSpan> {
    let mut matches = text.match_indices(marker);
    let (start, _) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(ByteSpan {
        start,
        end: start + marker.len(),
    })
}

fn bounded_text(text: &str, span: ByteSpan) -> (String, ByteSpan, bool) {
    bounded_text_around(text, span, span)
}

fn bounded_text_around(text: &str, span: ByteSpan, anchor: ByteSpan) -> (String, ByteSpan, bool) {
    let end = span.end.min(text.len());
    let start = span.start.min(end);
    let mut snippet_start = start;
    let mut snippet_end = end;
    if end - start > MAX_LOOKUP_SNIPPET_BYTES {
        let anchor_start = anchor.start.clamp(start, end);
        let anchor_end = anchor.end.clamp(anchor_start, end);
        let preceding = (MAX_LOOKUP_SNIPPET_BYTES.saturating_sub(anchor_end - anchor_start)) / 2;
        snippet_start = anchor_start
            .saturating_sub(preceding)
            .clamp(start, end - MAX_LOOKUP_SNIPPET_BYTES);
        snippet_end = snippet_start + MAX_LOOKUP_SNIPPET_BYTES;
        if anchor_end > snippet_end {
            snippet_start = anchor_end - MAX_LOOKUP_SNIPPET_BYTES;
            snippet_end = anchor_end;
        }
    }
    while !text.is_char_boundary(snippet_start) {
        snippet_start -= 1;
    }
    while !text.is_char_boundary(snippet_end) {
        snippet_end -= 1;
    }
    let truncated = snippet_start != start || snippet_end != end;
    (
        text[snippet_start..snippet_end].to_owned(),
        ByteSpan {
            start: snippet_start,
            end: snippet_end,
        },
        truncated,
    )
}

fn is_builtin_or_method(name: &str) -> bool {
    matches!(
        name,
        "Some" | "None" | "Ok" | "Err" | "format" | "println" | "assert" | "vec"
    )
}

struct SymbolVisitor<'a> {
    text: &'a str,
    symbols: Vec<RustSymbol>,
    scopes: Vec<String>,
    function_stack: Vec<Option<usize>>,
    next_scope: usize,
}

impl<'a> SymbolVisitor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            symbols: Vec::new(),
            scopes: Vec::new(),
            function_stack: Vec::new(),
            next_scope: 0,
        }
    }

    fn push(
        &mut self,
        name: String,
        kind: RustSymbolKind,
        span: proc_macro2::Span,
    ) -> Option<usize> {
        if let Some(span) = span_to_bytes(self.text, span.start(), span.end()) {
            let qualified_name = if self.scopes.is_empty() {
                name.clone()
            } else {
                format!("{}::{name}", self.scopes.join("::"))
            };
            self.symbols.push(RustSymbol {
                name,
                qualified_name,
                kind,
                span,
                parent: (!self.scopes.is_empty()).then(|| self.scopes.join("::")),
                references: Vec::new(),
            });
            Some(self.symbols.len() - 1)
        } else {
            None
        }
    }

    fn with_scope(&mut self, name: String, visit: impl FnOnce(&mut Self)) {
        self.scopes.push(name);
        visit(self);
        self.scopes.pop();
    }

    fn with_function_scope(
        &mut self,
        name: String,
        symbol: Option<usize>,
        visit: impl FnOnce(&mut Self),
    ) {
        self.scopes.push(name);
        self.function_stack.push(symbol);
        visit(self);
        self.function_stack.pop();
        self.scopes.pop();
    }

    fn without_function(&mut self, visit: impl FnOnce(&mut Self)) {
        let enclosing = std::mem::take(&mut self.function_stack);
        visit(self);
        self.function_stack = enclosing;
    }
}

fn span_to_bytes(text: &str, start: LineColumn, end: LineColumn) -> Option<ByteSpan> {
    fn offset(text: &str, location: LineColumn) -> Option<usize> {
        let line_start = if location.line == 1 {
            0
        } else {
            text.match_indices('\n')
                .nth(location.line - 2)
                .map(|(index, _)| index + 1)?
        };
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |offset| line_start + offset);
        let line = &text[line_start..line_end];
        let offset = line.char_indices().nth(location.column).map_or_else(
            || (line.chars().count() == location.column).then_some(line.len()),
            |(offset, _)| Some(offset),
        )?;
        let offset = line_start.checked_add(offset)?;
        (offset <= text.len() && text.is_char_boundary(offset)).then_some(offset)
    }
    let start = offset(text, start)?;
    let end = offset(text, end)?;
    (end >= start).then_some(ByteSpan { start, end })
}

impl<'ast> Visit<'ast> for SymbolVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let name = node.sig.ident.to_string();
        let symbol = self.push(name.clone(), RustSymbolKind::Function, node.span());
        self.with_function_scope(name, symbol, |this| syn::visit::visit_item_fn(this, node));
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let name = node.ident.to_string();
        self.push(name.clone(), RustSymbolKind::Struct, node.span());
        self.with_scope(name, |this| syn::visit::visit_item_struct(this, node));
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        let name = node.ident.to_string();
        self.push(name.clone(), RustSymbolKind::Enum, node.span());
        self.with_scope(name, |this| syn::visit::visit_item_enum(this, node));
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        let name = node.ident.to_string();
        self.push(name.clone(), RustSymbolKind::Trait, node.span());
        self.with_scope(name, |this| syn::visit::visit_item_trait(this, node));
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let name = format!("impl{}", self.next_scope);
        self.next_scope += 1;
        self.push(name.clone(), RustSymbolKind::Impl, node.span());
        self.with_scope(name, |this| syn::visit::visit_item_impl(this, node));
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        self.push(node.ident.to_string(), RustSymbolKind::Type, node.span());
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        self.push(
            node.ident.to_string(),
            RustSymbolKind::Constant,
            node.span(),
        );
        self.without_function(|this| syn::visit::visit_item_const(this, node));
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        self.push(node.ident.to_string(), RustSymbolKind::Static, node.span());
        self.without_function(|this| syn::visit::visit_item_static(this, node));
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let name = node.ident.to_string();
        self.push(name.clone(), RustSymbolKind::Module, node.span());
        if node.content.is_some() {
            self.with_scope(name, |this| syn::visit::visit_item_mod(this, node));
        }
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let name = node.sig.ident.to_string();
        let symbol = self.push(name.clone(), RustSymbolKind::Function, node.span());
        self.with_function_scope(name, symbol, |this| {
            syn::visit::visit_impl_item_fn(this, node)
        });
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        let name = node.sig.ident.to_string();
        let symbol = self.push(name.clone(), RustSymbolKind::Function, node.span());
        self.with_function_scope(name, symbol, |this| {
            syn::visit::visit_trait_item_fn(this, node)
        });
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let name = match node.func.as_ref() {
            syn::Expr::Path(path) => Some(
                path.path
                    .segments
                    .iter()
                    .map(|p| p.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::"),
            ),
            _ => None,
        };
        if let Some(name) = name
            && let Some(Some(owner)) = self.function_stack.last()
            && let Some(symbol) = self.symbols.get_mut(*owner)
        {
            symbol.references.push(name);
        }
        syn::visit::visit_expr_call(self, node);
    }
}

fn collect_symbols(syntax: &syn::File, text: &str) -> Vec<RustSymbol> {
    let mut visitor = SymbolVisitor::new(text);
    visitor.visit_file(syntax);
    for symbol in &mut visitor.symbols {
        symbol.references.sort();
        symbol.references.dedup();
    }
    visitor.symbols.sort_by(|a, b| {
        a.span
            .start
            .cmp(&b.span.start)
            .then(a.span.end.cmp(&b.span.end))
            .then(a.qualified_name.cmp(&b.qualified_name))
    });
    visitor.symbols
}
