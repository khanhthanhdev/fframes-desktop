//! Paint-only syntax highlighting for agent code blocks.
//!
//! [`highlight`] turns a fence tag and the code text into byte-range spans with a semantic
//! [`TokenClass`]. The renderer paints the spans as coloured text runs through [`token_color`];
//! the text and its layout are never touched. Five grammars are bundled: Rust, TOML, JSON,
//! Markdown and shell. Any other tag, and every tag when the `syntax-grammars` feature is off,
//! yields no spans.
//!
//! # Cost
//!
//! Compiled grammars and queries are built once per language, on first use. A call parses the
//! whole text again (no incremental reuse), so its cost is linear in the block size. A block
//! that grows while the model streams can be re-highlighted on every chunk, but the caller
//! should coalesce chunks to at most a few calls per frame for large blocks. Text above
//! [`MAX_HIGHLIGHT_BYTES`] returns no spans. A partial block (an unterminated fence) is parsed
//! with error recovery, so the finished part keeps its colours while the tail may be plain.

use std::ops::Range;

use gpui::Rgba;
use studio_theme::Theme;

#[cfg(feature = "syntax-grammars")]
mod engine;

/// Text longer than this many bytes is left uncoloured, which bounds the per-call cost.
pub const MAX_HIGHLIGHT_BYTES: usize = 128 * 1024;

/// The languages with a bundled grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Toml,
    Json,
    Markdown,
    Shell,
}

impl Language {
    /// The language for a fenced block's info string (`rs`, `Rust`, `rust,ignore`, ...).
    pub fn from_tag(tag: &str) -> Option<Self> {
        let word = tag
            .trim()
            .split(|c: char| c.is_whitespace() || matches!(c, ',' | '{' | ':'))
            .next()
            .unwrap_or("");
        match word.to_ascii_lowercase().as_str() {
            "rs" | "rust" => Some(Self::Rust),
            "toml" => Some(Self::Toml),
            "json" | "jsonc" => Some(Self::Json),
            "md" | "markdown" => Some(Self::Markdown),
            "sh" | "bash" | "shell" | "zsh" | "console" => Some(Self::Shell),
            _ => None,
        }
    }
}

/// What a span of code is, independent of any theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenClass {
    Comment,
    Keyword,
    String,
    StringSpecial,
    Escape,
    Number,
    Boolean,
    Type,
    TypeBuiltin,
    Constructor,
    Function,
    FunctionBuiltin,
    Macro,
    Property,
    Constant,
    Variable,
    VariableBuiltin,
    Parameter,
    Operator,
    Punctuation,
    Tag,
    Attribute,
    Label,
    Heading,
    Raw,
    Link,
    Reference,
    Emphasis,
    Strong,
    Embedded,
    Invalid,
}

/// A coloured byte range of the highlighted text. Ranges are ordered, never overlap and sit on
/// char boundaries; uncoloured text has no span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub class: TokenClass,
}

/// Spans for `code` in the language named by a fence tag; empty for an unknown tag.
pub fn highlight(tag: &str, code: &str) -> Vec<HighlightSpan> {
    match Language::from_tag(tag) {
        Some(language) => highlight_language(language, code),
        None => Vec::new(),
    }
}

/// Spans for `code` in `language`. Empty when the grammars are compiled out, the text is over
/// [`MAX_HIGHLIGHT_BYTES`], or the parser fails.
pub fn highlight_language(language: Language, code: &str) -> Vec<HighlightSpan> {
    if code.len() > MAX_HIGHLIGHT_BYTES {
        return Vec::new();
    }
    #[cfg(feature = "syntax-grammars")]
    {
        engine::highlight(language, code)
    }
    #[cfg(not(feature = "syntax-grammars"))]
    {
        let _ = language;
        Vec::new()
    }
}

/// The text colour for a token class under `theme`.
pub fn token_color(class: TokenClass, theme: &Theme) -> Rgba {
    use TokenClass::*;
    let syntax = &theme.syntax;
    match class {
        Comment => syntax.comment,
        Keyword | Operator | Tag => syntax.keyword,
        String | StringSpecial | Raw | Escape => syntax.string,
        Number | Boolean | Constant | Attribute | Embedded => syntax.number,
        Type | TypeBuiltin | Constructor | Label | Link | Reference => syntax.type_name,
        Function | FunctionBuiltin | Macro | Heading | Strong => syntax.function,
        Property | Variable | VariableBuiltin | Parameter | Emphasis => syntax.variable,
        Punctuation => syntax.punctuation,
        Invalid => syntax.invalid,
    }
}

#[cfg(test)]
mod tests;
