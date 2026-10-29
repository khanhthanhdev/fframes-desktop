//! Props hooks: how a video that has editable props lets the worker change them. The
//! worker does not know where the video keeps its props; the project's worker binary wires
//! a store's closures in with [`PreviewWorkerConfig::with_props`](crate::PreviewWorkerConfig::with_props).
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use fframes::{RenderOptions, Video};
use fframes_studio_protocol::{EditorObjectIdentity, PropsDocument, PropsInfo, StopSuggestions};

type ApplyFn = dyn Fn(Option<PropsDocument>) + Send + Sync;
type InfoFn = dyn Fn(&EditorObjectIdentity) -> Option<PropsInfo> + Send + Sync;
type SuggestionsFn = dyn Fn() -> StopSuggestions + Send + Sync;
type ClearSuggestionsFn = dyn Fn() + Send + Sync;

/// `apply` replaces the video's props document (`None` restores the code's values) and
/// takes effect from the next rendered frame. `info` describes an editable object as it
/// last rendered.
#[derive(Clone)]
pub struct PropsHooks {
    apply: Arc<ApplyFn>,
    info: Arc<InfoFn>,
    suggestions: Option<(Arc<SuggestionsFn>, Arc<ClearSuggestionsFn>)>,
}

impl fmt::Debug for PropsHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropsHooks").finish_non_exhaustive()
    }
}

impl PropsHooks {
    pub fn new(
        apply: impl Fn(Option<PropsDocument>) + Send + Sync + 'static,
        info: impl Fn(&EditorObjectIdentity) -> Option<PropsInfo> + Send + Sync + 'static,
    ) -> Self {
        Self {
            apply: Arc::new(apply),
            info: Arc::new(info),
            suggestions: None,
        }
    }

    /// Enables optional stop scans. Clearing starts a fresh scan without old observations.
    pub fn with_suggestions(
        mut self,
        read: impl Fn() -> StopSuggestions + Send + Sync + 'static,
        clear: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.suggestions = Some((Arc::new(read), Arc::new(clear)));
        self
    }

    pub fn supports_suggestions(&self) -> bool {
        self.suggestions.is_some()
    }

    pub fn suggested_stops(&self) -> StopSuggestions {
        self.suggestions
            .as_ref()
            .map_or_else(StopSuggestions::default, |(read, _)| read())
    }

    pub fn clear_suggestions(&self) {
        if let Some((_, clear)) = &self.suggestions {
            clear();
        }
    }

    pub fn apply(&self, doc: Option<PropsDocument>) {
        (self.apply)(doc)
    }

    pub fn info(&self, identity: &EditorObjectIdentity) -> Option<PropsInfo> {
        (self.info)(identity)
    }
}

/// Reads a props document; a missing file is `None`.
pub fn read_props_file(path: &Path) -> Result<Option<PropsDocument>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => PropsDocument::parse(&text)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub(crate) fn arg<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
}

/// [`run_export`](crate::run_export) with the props document of `--props-file <path>`
/// applied first, so the export draws what the preview showed. A `--props-file` that is
/// missing or invalid fails the export instead of silently exporting the code's values.
pub fn run_export_with_props<V: Video + Send>(
    video: &V,
    options: &RenderOptions<'_, '_>,
    args: &[String],
    props: &PropsHooks,
) -> Result<(), Box<dyn Error>> {
    if let Some(path) = arg(args, "--props-file") {
        let doc = read_props_file(Path::new(path))?
            .ok_or_else(|| format!("props file {path} does not exist"))?;
        props.apply(Some(doc));
    }
    crate::run_export(video, options, args)
}
