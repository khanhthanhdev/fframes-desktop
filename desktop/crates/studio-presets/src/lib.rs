//! GPUI-free, versioned style presets for fframes studio.
//!
//! * [`model`]: diagnostics, bounds, token names and typed values.
//! * [`resolve`]: token sets, alias resolution and preset -> project -> scene precedence.
//! * [`package`]: preset directory packages (verify, import, export, canonical hash).
//! * [`css_import`]: allowlisted CSS custom-property importer with a per-declaration report.
//! * [`builtin`]: the bundled, read-only presets.
//! * [`project`]: the project-local file set (`style/*`, preset media) the engine writes.

pub mod builtin;
pub mod css_import;
pub mod model;
pub mod package;
pub mod project;
pub mod resolve;

pub use css_import::{CssEntry, CssReport, Status as CssStatus, import_css};
pub use model::{
    Code, Color, Design, Diagnostic, Easing, PRESET_SCHEMA, PresetError, Severity, Shadow,
    TOKEN_SCHEMA, TokenDef, TokenKind, TokenName, TokenValue, Typography,
};
pub use package::{Manifest, Package, export_dir, import_dir};
pub use project::{MaterializedStyle, materialize, reresolve_project_style};
pub use resolve::{Layer, OverrideLayer, OverridesFile, ResolvedSnapshot, TokenSet, resolve};
