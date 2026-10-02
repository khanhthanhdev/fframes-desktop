//! Portable project metadata and byte-based source identity. No SDK or process state lives here.
pub mod assets;
pub mod checkpoint;
pub mod lifecycle;
pub mod manifest;
pub mod paths;
pub mod revision;

pub use lifecycle::{OpenProject, create, import, open, open_for_recovery};
pub use manifest::{Manifest, ProjectError, ProjectId};
pub use paths::ProjectPath;
pub use revision::{SourceInventory, SourceRevision};
