//! fframes Studio's side of fframes-kit's editable props.
//!
//! A [`PropsStore`] holds a project's `props.json`. Its views are the kit's
//! [`PropsSource`]: they name each editable object with Studio's editor identity (scene,
//! component, key, `main`), draw it inside a `<g>` whose id Studio's canvas selects, and,
//! while the store listens, record how it rendered for the props panel.
//!
//! ```ignore
//! // render_frame of a video, and of a scene
//! let mut ui = Ui::new(frame, ctx, &self.theme).with_props(self.props.view());
//! let mut ui = Ui::for_scene(frame, ctx, theme, self).with_props(props.scene_view(self));
//!
//! // a kit deck: every slide's Ui
//! let deck = Deck::new(theme, slides).with_props(props.clone(), PropsStore::slide_view);
//!
//! // the project's Studio worker
//! props.listen();
//! let config = config.with_props_args(&args).with_props(props.hooks());
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use fframes::{EditorObjectKey, Scene};
use fframes_kit::props::{EditableObject, PropValues, PropsSource};
use fframes_studio_protocol::{
    EditorObjectIdentity, PropField, PropKind, PropsDocument, PropsInfo, PropsSchema,
    StopSuggestions, SuggestedSlideStop,
};
use fframes_studio_runtime::PropsHooks;

/// The repeat part of every kit identity: keys are unique per scene.
pub const REPEAT_KEY: &str = "main";

/// The scene part of identities outside scenes.
pub const VIDEO_SCENE_KEY: &str = "video";

/// The props document of a video plus, while Studio listens, how each editable object
/// last rendered. Shared between render threads; keep it in an `Arc` in the video.
#[derive(Debug, Default)]
pub struct PropsStore {
    doc: RwLock<Arc<PropsDocument>>,
    rendered: Mutex<HashMap<EditorObjectIdentity, PropsInfo>>,
    suggested: Mutex<Suggestions>,
    listening: AtomicBool,
}

#[derive(Debug, Default)]
struct Suggestions {
    origins: BTreeMap<(String, String), Vec<SuggestedSlideStop>>,
    truncated: bool,
}

impl PropsStore {
    pub fn new(doc: PropsDocument) -> Self {
        Self {
            doc: RwLock::new(Arc::new(doc)),
            ..Default::default()
        }
    }

    /// The document in `path`; a missing file is an empty document.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        match std::fs::read_to_string(path) {
            Ok(text) => PropsDocument::parse(&text)
                .map(Self::new)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    /// Replaces the document from the next `Ui` on; `None` restores the code's values.
    pub fn set(&self, doc: Option<PropsDocument>) {
        *self.doc.write().unwrap_or_else(PoisonError::into_inner) =
            Arc::new(doc.unwrap_or_default());
    }

    pub fn document(&self) -> Arc<PropsDocument> {
        self.doc
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records how editable objects render from now on, for [`info`](Self::info).
    pub fn listen(&self) {
        self.listening.store(true, Ordering::Relaxed);
    }

    /// The schema and values of an object as it last rendered; `None` before
    /// [`listen`](Self::listen) or when it has not rendered since.
    pub fn info(&self, identity: &EditorObjectIdentity) -> Option<PropsInfo> {
        self.rendered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(identity)
            .cloned()
    }

    /// The props of a video's `Ui` (scene key `video`), read once here.
    pub fn view(&self) -> PropsView<'_> {
        self.view_in(VIDEO_SCENE_KEY)
    }

    /// The props of a scene's `Ui`: the scene key is its `editor_instance_key`, else its
    /// name, as in Studio's timeline.
    pub fn scene_view(&self, scene: &dyn Scene) -> PropsView<'_> {
        self.view_in(scene.editor_instance_key().unwrap_or_else(|| scene.name()))
    }

    /// [`scene_view`](Self::scene_view) as the view of a kit `Deck`:
    /// `deck.with_props(store.clone(), PropsStore::slide_view)`.
    pub fn slide_view<'s>(store: &'s Self, scene: &dyn Scene) -> Box<dyn PropsSource + 's> {
        Box::new(store.scene_view(scene))
    }

    fn view_in(&self, scene: &str) -> PropsView<'_> {
        PropsView {
            store: self,
            doc: self.document(),
            scene: scene.to_owned(),
        }
    }

    /// The hooks that hand this store to Studio's preview worker and exports.
    pub fn hooks(self: &Arc<Self>) -> PropsHooks {
        let (apply, info) = (self.clone(), self.clone());
        let (suggestions, clear) = (self.clone(), self.clone());
        PropsHooks::new(
            move |doc| apply.set(doc),
            move |identity| info.info(identity),
        )
        .with_suggestions(
            move || suggestions.suggested_stops(),
            move || clear.clear_suggestions(),
        )
    }

    /// The latest bounded reports, replaced per scene and component origin.
    pub fn suggested_stops(&self) -> StopSuggestions {
        let suggested = self.suggested.lock().expect("suggestions lock poisoned");
        StopSuggestions {
            stops: suggested.origins.values().flatten().cloned().collect(),
            truncated: suggested.truncated,
        }
    }

    /// Starts a fresh scan, discarding earlier render observations.
    pub fn clear_suggestions(&self) {
        *self.suggested.lock().expect("suggestions lock poisoned") = Suggestions::default();
    }

    fn record(&self, identity: EditorObjectIdentity, info: PropsInfo) {
        self.rendered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity, info);
    }
}

/// One snapshot of a [`PropsStore`]'s document for the editable objects of one scene.
#[derive(Debug)]
pub struct PropsView<'s> {
    store: &'s PropsStore,
    doc: Arc<PropsDocument>,
    scene: String,
}

impl PropsView<'_> {
    pub fn identity(&self, object: EditableObject<'_>) -> EditorObjectIdentity {
        EditorObjectIdentity {
            scene_instance_key: self.scene.clone(),
            component_key: object.component.to_owned(),
            object_key: object.key.to_owned(),
            repeat_key: REPEAT_KEY.to_owned(),
        }
    }
}

impl PropsSource for PropsView<'_> {
    fn values(&self, object: EditableObject<'_>) -> Option<&PropValues> {
        self.doc.values(&self.identity(object))
    }

    fn listening(&self) -> bool {
        self.store.listening.load(Ordering::Relaxed)
    }

    fn rendered(&self, object: EditableObject<'_>, info: fframes_kit::props::PropsInfo) {
        self.store.record(self.identity(object), props_info(info));
    }

    fn element_id(&self, object: EditableObject<'_>) -> Option<String> {
        EditorObjectKey::new(&self.scene, object.component, object.key, REPEAT_KEY)
            .ok()
            .map(|key| key.render_id())
    }
}

fn props_info(info: fframes_kit::props::PropsInfo) -> PropsInfo {
    PropsInfo {
        schema: props_schema(info.schema),
        code_values: info.code_values,
        values: info.values,
    }
}

fn props_schema(schema: fframes_kit::props::PropsSchema) -> PropsSchema {
    PropsSchema {
        component: schema.component,
        fields: schema
            .fields
            .into_iter()
            .map(|field| PropField {
                name: field.name,
                kind: prop_kind(field.kind),
                min: field.min,
                max: field.max,
                step: field.step,
                unit: field.unit,
            })
            .collect(),
    }
}

fn prop_kind(kind: fframes_kit::props::PropKind) -> PropKind {
    use fframes_kit::props::PropKind as Kit;
    match kind {
        Kit::Number => PropKind::Number,
        Kit::Integer => PropKind::Integer,
        Kit::Text => PropKind::Text,
        Kit::Color => PropKind::Color,
        Kit::Toggle => PropKind::Toggle,
        Kit::Choice { options } => PropKind::Choice { options },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn the_store_swaps_documents_whole() {
        let store = PropsStore::default();
        let identity = EditorObjectIdentity {
            scene_instance_key: "s".into(),
            component_key: "C".into(),
            object_key: "o".into(),
            repeat_key: REPEAT_KEY.into(),
        };
        let before = store.document();
        let mut doc = PropsDocument::default();
        doc.set(&identity, "x", Some(Value::from(1)));
        store.set(Some(doc.clone()));
        assert_eq!(*store.document(), doc);
        assert_eq!(
            *before,
            PropsDocument::default(),
            "snapshots keep their document"
        );
        store.set(None);
        assert_eq!(*store.document(), PropsDocument::default());
    }

    #[test]
    fn kit_kinds_map_to_protocol_kinds() {
        use fframes_kit::props::PropKind as Kit;
        let options = vec!["Left".to_owned(), "Right".to_owned()];
        let pairs = [
            (Kit::Number, PropKind::Number),
            (Kit::Integer, PropKind::Integer),
            (Kit::Text, PropKind::Text),
            (Kit::Color, PropKind::Color),
            (Kit::Toggle, PropKind::Toggle),
            (
                Kit::Choice {
                    options: options.clone(),
                },
                PropKind::Choice { options },
            ),
        ];
        for (kit, protocol) in pairs {
            assert_eq!(prop_kind(kit), protocol);
        }
    }
}
