pub mod agent_tools;
pub mod agent_workflow;
pub mod app;
pub mod audio_service;
pub mod build_service;
pub mod candidate_runner;
pub mod canvas_view;
pub mod conversation_panel;
pub mod design_system;
mod evidence_preview;
pub mod export_service;
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
pub mod preset_panel;
pub mod preview_coordinator;
pub mod preview_element;
pub mod preview_worker_client;
pub mod thumbnail_cache;
pub mod timeline_view;

pub mod agent_spike;
pub mod project_view;
pub mod stress_metrics;
pub mod studio_shell;
pub mod teardown;
pub mod worker_project;
