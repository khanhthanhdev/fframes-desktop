//! One file per section of the video. Every scene declares the beats it
//! starts and ends on; `lb` is the beat position inside the scene.

/// A scene struct with its place on the beat grid. `None` means the start or
/// the end of the video.
macro_rules! beat_scene {
    ($name:ident, $start:expr, $end:expr) => {
        #[derive(Debug)]
        pub struct $name;
        impl $name {
            pub const START: Option<f32> = $start;
            pub const END: Option<f32> = $end;
            /// Beat inside the scene (0 on its first downbeat).
            #[allow(dead_code)]
            pub fn lb(frame: &fframes::Frame) -> f32 {
                crate::beat::gbeat(frame) - Self::START.unwrap_or(0.0)
            }
            pub fn frames() -> fframes::Duration<'static> {
                fframes::Duration::Frames(crate::beat::span_frames(Self::START, Self::END))
            }
        }
    };
}

pub mod first_run;
pub mod hook;
pub mod install;
pub mod outro;
pub mod parallel;
pub mod portrait;
pub mod project;
pub mod story;
pub mod studio;

pub use hook::HookScene;
pub use install::InstallScene;
pub use outro::{EndScene, OutroScene};
pub use parallel::ParallelScene;
pub use project::ProjectScene;
pub use story::{FastScene, HowScene, WhyScene};
pub use studio::StudioScene;
