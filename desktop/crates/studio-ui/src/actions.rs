//! The studio's commands as gpui actions. The shell handles every one at its root, so an
//! action dispatched anywhere in the window (a key binding, a menu, a host driving the
//! app) reaches the same code as the button that offers it.
use gpui::{Action, actions};

actions!(
    studio,
    [
        /// Choose a project folder and open it.
        OpenProject,
        /// Build the open project's preview.
        BuildPreview,
        /// Stop the preview build in flight.
        CancelPreview,
        /// Open the export panel for the accepted revision.
        ExportVideo,
        /// Choose a destination and export with the panel's settings.
        StartExport,
        /// Stop the export in flight.
        CancelExport,
        /// Play from the playhead, or pause.
        TogglePlayback,
        /// Pause and show the next frame.
        StepForward,
        /// Pause and show the previous frame.
        StepBackward,
        /// Mute or unmute the preview's audio.
        ToggleMute,
        /// Play through the next audio output: the default, each device, then none.
        NextAudioOutput,
        /// Play through the system's default audio output again.
        RetryAudioOutput,
        /// Play without audio, on the monotonic clock.
        DisableAudioOutput,
        /// Move the playhead to the first frame.
        SeekToStart,
        /// Move the playhead past the last frame.
        SeekToEnd,
        /// Select the next scene under the playhead on the timeline.
        CycleScene,
        /// Split the clip under the playhead in two.
        SplitAtPlayhead,
        /// Undo the last timeline or slide stop edit.
        UndoTimelineEdit,
        /// Redo the timeline or slide stop edit last undone.
        RedoTimelineEdit,
        /// Add a slide stop at the playhead.
        AddStopAtPlayhead,
        /// Remove the slide stop whose timeline marker was selected last.
        RemoveStop,
        /// Present the video as a slide deck, full screen, from its first frame.
        Present,
        /// Presenting: play the next slide, or finish the one playing.
        PresentNext,
        /// Presenting: hold at the stop before.
        PresentPrevious,
        /// Presenting: hold at the start of the first slide.
        PresentFirst,
        /// Presenting: hold at the start of the last slide.
        PresentLast,
        /// Leave the presentation and return to editing.
        ExitPresent,
        /// Start the timeline range selection at the playhead.
        MarkRangeStart,
        /// End the timeline range selection at the playhead.
        MarkRangeEnd,
        /// Zoom the timeline in, around the playhead.
        ZoomIn,
        /// Zoom the timeline out, around the playhead.
        ZoomOut,
        /// Scroll the timeline back by three quarters of its width.
        PageTimelineBack,
        /// Scroll the timeline forward by three quarters of its width.
        PageTimelineForward,
        /// Select the next object under the last canvas click.
        CycleCanvasSelection,
        /// Clear the canvas selection; the next task takes the timeline's selection.
        ClearCanvasSelection,
        /// Fit the video in the preview again.
        ResetCanvasView,
        /// Save the project's current source as its checkpoint.
        SaveCheckpoint,
        /// Copy the last checkpoint into a new folder of the user's choice.
        RestoreCheckpoint,
        /// Choose an installed SDK folder for the open project.
        SelectSdk,
        /// Choose an offline SDK bundle folder and install it as the managed SDK.
        ImportSdkBundle,
        /// Look for installed agents and their ACP adapters, without starting them.
        DetectAgents,
        /// Open the settings frame on the agent's setup page.
        OpenAgentSetup,
        /// Move the keyboard focus to the agent prompt.
        FocusPrompt,
        /// Move the keyboard focus to the preview.
        FocusPreview,
        /// Move the keyboard focus to the timeline's tracks.
        FocusTimeline,
        /// Send the agent prompt as a brief (or a clarification reply, or queue it).
        SubmitPrompt,
        /// Publish the candidate waiting for review.
        ApplyCandidate,
        /// Switch later agent tasks between automatic Apply and review before Apply.
        ToggleReviewPolicy,
    ]
);

/// Select the canvas object painted at video pixel (`x`, `y`) of the shown frame, as a
/// click there does; `cycle` selects the next object under that point instead.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct SelectCanvasAt {
    pub x: f64,
    pub y: f64,
    pub cycle: bool,
}

/// Apply the catalog preset `key` (its package hash) to the project, keeping its overrides.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct ApplyPreset {
    pub key: String,
}

/// Apply the catalog preset `key` and clear every project override.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct ResetPreset {
    pub key: String,
}

/// Override the applied preset's `token` with `value` (JSON when it parses, else text).
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct SetStyleOverride {
    pub token: String,
    pub value: String,
}

/// Answer the oldest open permission request with `option` (one of the ids it offers),
/// or decline it when `None`.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct AnswerPermission {
    pub option: Option<String>,
}

/// Turn on the agent `profile` with the adapter Detect agents found for it.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct UseDetectedAgent {
    pub profile: String,
}

/// Move the playhead to `frame`.
#[derive(Clone, PartialEq, Eq, Debug, Action)]
#[action(namespace = studio, no_json)]
pub struct Seek {
    pub frame: usize,
}
