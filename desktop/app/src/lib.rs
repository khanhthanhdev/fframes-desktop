pub mod app;
pub mod frame_image;
pub mod selection_spike;
pub mod setup_view;
pub mod text_input;
pub mod worker_client;

pub use app::StudioSpikeApp;
pub use frame_image::{
    ConversionError, ImagePresentationManager, convert_rgba_to_gpui_bgra, create_render_image,
    generate_reference_frame,
};
pub use selection_spike::{SelectedElementInfo, SelectionError, SelectionSpike};
pub use setup_view::{SetupState, SetupView};
pub use text_input::TextInput;
pub use worker_client::WorkerClient;

pub mod presentation_stress;
pub mod preview_element;

pub mod agent_spike;
pub mod stress_metrics;
pub mod worker_project;
