//! The shell's keymap and action handlers: every studio action dispatched to the window
//! lands here.
//!
//! Key contexts, most specific first: `TextInput` (a focused field, bound by the kit) and
//! `AgentComposer` (the agent prompt, bound by the composer), then
//! `Player` (the preview surface), `Timeline` (its tracks) and `Present` (the presentation
//! stage, which replaces both while presenting), then `Workspace` (the window). Plain keys
//! are bound only in `Player`, `Timeline` and `Present`, which hold no text field, so typing
//! in a field never reaches them; `Workspace` has no bindings of its own.

use gpui::{App, Context, InteractiveElement, KeyBinding};
use studio_engine::engine::backend::Command;
use studio_engine::presets::PresetCommand;
use studio_engine::services::audio_service::OutputDevice;
use studio_engine::timing::TimelineCommand;

use super::left_column::SidebarTab;
use super::{Picker, StudioShell};
use crate::actions;

/// The preview surface's key context.
pub(super) const PLAYER: &str = "Player";
/// The timeline tracks' key context.
const TIMELINE: &str = crate::timeline::KEY_CONTEXT;
/// The presentation stage's key context. The stage has no `Player` context, so its
/// arrows, space, Home, End and Escape navigate the deck.
pub(super) const PRESENT: &str = "Present";
/// The window's key context, around every other one.
pub(super) const WORKSPACE: &str = "Workspace";
/// The window's keys that must leave a focused text field alone.
const WORKSPACE_OUTSIDE_FIELDS: &str = "Workspace && !TextInput && !AgentComposer";

/// Binds the studio window's keys: the text fields' editing keys, then the player's and
/// the timeline's. Call once at startup, before the window opens.
pub fn bind_keys(cx: &mut App) {
    crate::kit::bind_text_input_keys(cx);
    crate::kit::bind_selectable_text_keys(cx);
    crate::agent::composer::keymap::init(cx);
    cx.bind_keys(key_bindings());
}

/// Where a shortcut works: its key context, and its group on Settings → Keyboard
/// Shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutScope {
    Player,
    Timeline,
    Present,
    /// The window, outside its text fields.
    Anywhere,
}

impl ShortcutScope {
    /// The groups in the order the settings page shows them.
    pub(super) const ALL: [ShortcutScope; 4] = [
        ShortcutScope::Player,
        ShortcutScope::Timeline,
        ShortcutScope::Present,
        ShortcutScope::Anywhere,
    ];

    /// The key context the scope's bindings are bound in.
    pub(super) fn context(self) -> &'static str {
        match self {
            ShortcutScope::Player => PLAYER,
            ShortcutScope::Timeline => TIMELINE,
            ShortcutScope::Present => PRESENT,
            ShortcutScope::Anywhere => WORKSPACE_OUTSIDE_FIELDS,
        }
    }

    /// The group's heading on the settings page.
    pub(super) fn label(self) -> &'static str {
        match self {
            ShortcutScope::Player => "Player",
            ShortcutScope::Timeline => "Timeline",
            ShortcutScope::Present => "Present",
            ShortcutScope::Anywhere => "Anywhere",
        }
    }

    /// When the group's keys work.
    pub(super) fn description(self) -> &'static str {
        match self {
            ShortcutScope::Player => "While the preview has focus.",
            ShortcutScope::Timeline => "While the timeline has focus.",
            ShortcutScope::Present => "While presenting the slides.",
            ShortcutScope::Anywhere => "Anywhere in the window outside a text field.",
        }
    }
}

/// One key binding of the shell: what it does, its keys and where. [`SHORTCUTS`] is the one
/// list both the keymap and Settings → Keyboard Shortcuts are made from.
pub(super) struct Shortcut {
    pub(super) scope: ShortcutScope,
    /// The keystrokes in gpui's syntax (`secondary-shift-z`).
    pub(super) keys: &'static str,
    /// What the keys do, as the settings page names it.
    pub(super) label: &'static str,
    bind: fn(&str, Option<&str>) -> KeyBinding,
}

const fn shortcut<A: gpui::Action + Default>(
    scope: ShortcutScope,
    keys: &'static str,
    label: &'static str,
) -> Shortcut {
    Shortcut {
        scope,
        keys,
        label,
        bind: bind::<A>,
    }
}

fn bind<A: gpui::Action + Default>(keys: &str, context: Option<&str>) -> KeyBinding {
    KeyBinding::new(keys, A::default(), context)
}

use ShortcutScope::{Anywhere, Player, Present, Timeline};

/// Every key the shell binds. Bindings that share a scope and a label are one row on the
/// settings page, their keys side by side.
pub(super) const SHORTCUTS: &[Shortcut] = &[
    shortcut::<actions::TogglePlayback>(Player, "space", "Play or pause"),
    shortcut::<actions::StepBackward>(Player, "left", "Step back one frame"),
    shortcut::<actions::StepForward>(Player, "right", "Step forward one frame"),
    shortcut::<actions::SeekToStart>(Player, "home", "Go to the start"),
    shortcut::<actions::SeekToEnd>(Player, "end", "Go to the end"),
    shortcut::<actions::CycleCanvasSelection>(
        Player,
        "c",
        "Select the next object under the pointer",
    ),
    shortcut::<actions::ResetCanvasView>(Player, "0", "Reset the canvas view"),
    shortcut::<actions::ClearCanvasSelection>(Player, "escape", "Clear the selection"),
    shortcut::<actions::TogglePlayback>(Timeline, "space", "Play or pause"),
    shortcut::<actions::ToggleMute>(Timeline, "m", "Mute or unmute"),
    shortcut::<actions::StepBackward>(Timeline, "left", "Step back one frame"),
    shortcut::<actions::StepForward>(Timeline, "right", "Step forward one frame"),
    shortcut::<actions::SeekToStart>(Timeline, "home", "Go to the start"),
    shortcut::<actions::SeekToEnd>(Timeline, "end", "Go to the end"),
    shortcut::<actions::ZoomIn>(Timeline, "+", "Zoom in"),
    shortcut::<actions::ZoomIn>(Timeline, "=", "Zoom in"),
    shortcut::<actions::ZoomIn>(Timeline, "shift-=", "Zoom in"),
    shortcut::<actions::ZoomOut>(Timeline, "-", "Zoom out"),
    shortcut::<actions::PageTimelineBack>(Timeline, "pageup", "Scroll back a page"),
    shortcut::<actions::PageTimelineForward>(Timeline, "pagedown", "Scroll forward a page"),
    shortcut::<actions::CycleScene>(Timeline, "s", "Select the next scene"),
    shortcut::<actions::SplitAtPlayhead>(Timeline, "c", "Split at the playhead"),
    shortcut::<actions::UndoTimelineEdit>(Timeline, "secondary-z", "Undo a timeline edit"),
    shortcut::<actions::RedoTimelineEdit>(Timeline, "secondary-shift-z", "Redo a timeline edit"),
    shortcut::<actions::UndoTimelineEdit>(Anywhere, "secondary-z", "Undo a timeline edit"),
    shortcut::<actions::RedoTimelineEdit>(Anywhere, "secondary-shift-z", "Redo a timeline edit"),
    shortcut::<actions::MarkRangeStart>(Timeline, "[", "Mark the range's start"),
    shortcut::<actions::MarkRangeEnd>(Timeline, "]", "Mark the range's end"),
    shortcut::<actions::AddStopAtPlayhead>(Timeline, "k", "Add a slide stop at the playhead"),
    shortcut::<actions::RemoveStop>(Timeline, "delete", "Remove the selected stop"),
    shortcut::<actions::RemoveStop>(Timeline, "backspace", "Remove the selected stop"),
    shortcut::<actions::Present>(Anywhere, "f5", "Present"),
    shortcut::<actions::PresentNext>(Present, "right", "Next slide"),
    shortcut::<actions::PresentNext>(Present, "space", "Next slide"),
    shortcut::<actions::PresentNext>(Present, "pagedown", "Next slide"),
    shortcut::<actions::PresentPrevious>(Present, "left", "Previous slide"),
    shortcut::<actions::PresentPrevious>(Present, "pageup", "Previous slide"),
    shortcut::<actions::PresentFirst>(Present, "home", "First slide"),
    shortcut::<actions::PresentLast>(Present, "end", "Last slide"),
    shortcut::<actions::ExitPresent>(Present, "escape", "Leave the presentation"),
];

/// The shell's key bindings, made from [`SHORTCUTS`].
pub(super) fn key_bindings() -> Vec<KeyBinding> {
    SHORTCUTS
        .iter()
        .map(|shortcut| (shortcut.bind)(shortcut.keys, Some(shortcut.scope.context())))
        .collect()
}

impl StudioShell {
    /// Every studio action, handled at the root so it reaches the shell from anywhere in
    /// the window.
    pub(super) fn on_actions(root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|s, _: &actions::OpenProject, _, cx| s.pick(Picker::Open, cx)))
            .on_action(
                cx.listener(|s, _: &actions::BuildPreview, _, cx| s.dispatch(Command::Build, cx)),
            )
            .on_action(cx.listener(|s, _: &actions::CancelPreview, _, cx| {
                s.dispatch(Command::CancelBuild, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::ExportVideo, _, cx| s.open_export(cx)))
            .on_action(cx.listener(|s, _: &actions::StartExport, _, cx| s.request_export(cx)))
            .on_action(cx.listener(|s, _: &actions::CancelExport, _, cx| s.cancel_export(cx)))
            .on_action(cx.listener(|s, action: &actions::ApplyPreset, _, cx| {
                let key = action.key.clone();
                s.run_preset(PresetCommand::Apply { key }, cx)
            }))
            .on_action(cx.listener(|s, action: &actions::ResetPreset, _, cx| {
                let key = action.key.clone();
                s.run_preset(PresetCommand::Reset { key }, cx)
            }))
            .on_action(cx.listener(|s, action: &actions::SetStyleOverride, _, cx| {
                let (token, value) = (action.token.clone(), action.value.clone());
                s.run_preset(PresetCommand::SetOverride { token, value }, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::SaveCheckpoint, _, cx| {
                s.dispatch(Command::Checkpoint, cx)
            }))
            .on_action(
                cx.listener(|s, _: &actions::RestoreCheckpoint, _, cx| s.pick(Picker::Restore, cx)),
            )
            .on_action(cx.listener(|s, _: &actions::SelectSdk, _, cx| s.pick(Picker::Sdk, cx)))
            .on_action(
                cx.listener(|s, _: &actions::ImportSdkBundle, _, cx| s.pick(Picker::SdkBundle, cx)),
            )
            .on_action(cx.listener(|s, _: &actions::DetectAgents, _, cx| {
                s.panel.update(cx, |panel, cx| panel.detect_agents(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::OpenAgentSetup, window, cx| {
                s.open_agent_settings(super::AgentTab::Agents, None, window, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::FocusPrompt, window, cx| {
                s.select_sidebar_tab(SidebarTab::Agent, cx);
                s.panel
                    .update(cx, |panel, cx| panel.focus_prompt(window, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::FocusPreview, window, cx| {
                window.focus(&s.preview_focus, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::FocusTimeline, window, cx| {
                s.timeline.update(cx, |t, cx| t.focus(window, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::SubmitPrompt, _, cx| {
                s.panel.update(cx, |panel, cx| panel.submit(cx))
            }))
            .on_action(cx.listener(|s, action: &actions::AnswerPermission, _, cx| {
                let option = action.option.clone();
                s.panel
                    .update(cx, |panel, cx| panel.answer_oldest_permission(option, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::ApplyCandidate, _, cx| {
                s.panel.update(cx, |panel, cx| panel.apply(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::ToggleReviewPolicy, _, cx| {
                s.panel
                    .update(cx, |panel, cx| panel.toggle_review_policy(cx))
            }))
            .on_action(cx.listener(|s, action: &actions::UseDetectedAgent, _, cx| {
                s.panel
                    .update(cx, |panel, cx| panel.use_detected(&action.profile, cx))
            }))
            .on_action(cx.listener(|s, action: &actions::SelectCanvasAt, _, cx| {
                s.pause_at_painted_frame(cx);
                match s.video_to_canvas(action.x, action.y) {
                    Some(point) => s.select_canvas(point, action.cycle, cx),
                    None => {
                        s.canvas.message = Some("The video is not on the canvas yet.".into());
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|s, _: &actions::TogglePlayback, _, cx| s.toggle_playback(cx)))
            .on_action(cx.listener(|s, _: &actions::StepForward, _, cx| s.step_preview(1, cx)))
            .on_action(cx.listener(|s, _: &actions::StepBackward, _, cx| s.step_preview(-1, cx)))
            .on_action(cx.listener(|s, _: &actions::ToggleMute, _, cx| s.toggle_mute(cx)))
            .on_action(cx.listener(|s, _: &actions::NextAudioOutput, _, cx| s.cycle_output(cx)))
            .on_action(cx.listener(|s, _: &actions::RetryAudioOutput, _, cx| {
                s.change_output(OutputDevice::Default, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::DisableAudioOutput, _, cx| {
                s.change_output(OutputDevice::Unavailable, cx)
            }))
            .on_action(cx.listener(|s, action: &actions::Seek, _, cx| {
                s.timeline.update(cx, |t, cx| t.seek(action.frame, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::SeekToStart, _, cx| {
                s.timeline.update(cx, |t, cx| t.seek(0, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::SeekToEnd, _, cx| {
                s.timeline.update(cx, |t, cx| t.seek_to_end(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::CycleScene, _, cx| {
                s.timeline.update(cx, |t, cx| t.cycle_scene(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::SplitAtPlayhead, _, cx| {
                s.timeline.update(cx, |t, cx| t.split_at_playhead(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::UndoTimelineEdit, _, cx| {
                s.timeline_history(TimelineCommand::Undo, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::RedoTimelineEdit, _, cx| {
                s.timeline_history(TimelineCommand::Redo, cx)
            }))
            .on_action(cx.listener(|s, _: &actions::AddStopAtPlayhead, _, cx| {
                s.timeline.update(cx, |t, cx| t.add_stop_at_playhead(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::RemoveStop, _, cx| {
                s.timeline.update(cx, |t, cx| t.remove_selected_stop(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::Present, _, cx| s.present(None, cx)))
            .on_action(cx.listener(|s, _: &actions::PresentNext, _, cx| s.present_next(cx)))
            .on_action(cx.listener(|s, _: &actions::PresentPrevious, _, cx| s.present_previous(cx)))
            .on_action(cx.listener(|s, _: &actions::PresentFirst, _, cx| s.present_slide(1, cx)))
            .on_action(
                cx.listener(|s, _: &actions::PresentLast, _, cx| s.present_slide(usize::MAX, cx)),
            )
            .on_action(cx.listener(|s, _: &actions::ExitPresent, _, cx| s.exit_present(cx)))
            .on_action(cx.listener(|s, _: &actions::MarkRangeStart, _, cx| {
                s.timeline.update(cx, |t, cx| t.mark_range_start(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::MarkRangeEnd, _, cx| {
                s.timeline.update(cx, |t, cx| t.mark_range_end(cx))
            }))
            .on_action(cx.listener(|s, _: &actions::ZoomIn, _, cx| {
                s.timeline.update(cx, |t, cx| t.zoom(2., cx))
            }))
            .on_action(cx.listener(|s, _: &actions::ZoomOut, _, cx| {
                s.timeline.update(cx, |t, cx| t.zoom(0.5, cx))
            }))
            .on_action(cx.listener(|s, _: &actions::PageTimelineBack, _, cx| {
                s.timeline.update(cx, |t, cx| t.page(-1., cx))
            }))
            .on_action(cx.listener(|s, _: &actions::PageTimelineForward, _, cx| {
                s.timeline.update(cx, |t, cx| t.page(1., cx))
            }))
            .on_action(cx.listener(|s, _: &actions::CycleCanvasSelection, _, cx| {
                s.cycle_canvas_selection(cx)
            }))
            .on_action(cx.listener(|s, _: &actions::ClearCanvasSelection, _, cx| {
                s.clear_canvas_selection(cx)
            }))
            .on_action(
                cx.listener(|s, _: &actions::ResetCanvasView, _, cx| s.reset_canvas_view(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound() -> Vec<(String, String, &'static str)> {
        key_bindings()
            .iter()
            .map(|binding| {
                let context = binding
                    .predicate()
                    .map(|predicate| predicate.to_string())
                    .unwrap_or_default();
                let keys = binding
                    .keystrokes()
                    .iter()
                    .map(|keystroke| keystroke.inner().unparse())
                    .collect::<Vec<_>>()
                    .join(" ");
                (context, keys, binding.action().name())
            })
            .collect()
    }

    #[test]
    fn every_shell_binding_is_scoped_to_the_player_or_the_timeline() {
        // A binding in a wider context would win over a focused text field's typing.
        for (context, keys, action) in bound() {
            assert!(
                context == PLAYER
                    || context == TIMELINE
                    || context == PRESENT
                    || context == WORKSPACE_OUTSIDE_FIELDS,
                "{keys} -> {action} is bound in {context:?}"
            );
        }
    }

    #[test]
    fn the_player_and_the_timeline_answer_their_keys() {
        let bindings = bound();
        for (context, keys, action) in [
            (PLAYER, "space", "studio::TogglePlayback"),
            (PLAYER, "left", "studio::StepBackward"),
            (PLAYER, "right", "studio::StepForward"),
            (PLAYER, "home", "studio::SeekToStart"),
            (PLAYER, "end", "studio::SeekToEnd"),
            (PLAYER, "c", "studio::CycleCanvasSelection"),
            (PLAYER, "0", "studio::ResetCanvasView"),
            (PLAYER, "escape", "studio::ClearCanvasSelection"),
            (TIMELINE, "space", "studio::TogglePlayback"),
            (TIMELINE, "m", "studio::ToggleMute"),
            (TIMELINE, "left", "studio::StepBackward"),
            (TIMELINE, "right", "studio::StepForward"),
            (TIMELINE, "home", "studio::SeekToStart"),
            (TIMELINE, "end", "studio::SeekToEnd"),
            (TIMELINE, "+", "studio::ZoomIn"),
            (TIMELINE, "=", "studio::ZoomIn"),
            (TIMELINE, "shift-=", "studio::ZoomIn"),
            (TIMELINE, "-", "studio::ZoomOut"),
            (TIMELINE, "pageup", "studio::PageTimelineBack"),
            (TIMELINE, "pagedown", "studio::PageTimelineForward"),
            (TIMELINE, "s", "studio::CycleScene"),
            (TIMELINE, "c", "studio::SplitAtPlayhead"),
            (TIMELINE, "secondary-z", "studio::UndoTimelineEdit"),
            (TIMELINE, "secondary-shift-z", "studio::RedoTimelineEdit"),
            (
                WORKSPACE_OUTSIDE_FIELDS,
                "secondary-z",
                "studio::UndoTimelineEdit",
            ),
            (
                WORKSPACE_OUTSIDE_FIELDS,
                "secondary-shift-z",
                "studio::RedoTimelineEdit",
            ),
            (TIMELINE, "[", "studio::MarkRangeStart"),
            (TIMELINE, "]", "studio::MarkRangeEnd"),
            (TIMELINE, "k", "studio::AddStopAtPlayhead"),
            (TIMELINE, "delete", "studio::RemoveStop"),
            (TIMELINE, "backspace", "studio::RemoveStop"),
            (WORKSPACE_OUTSIDE_FIELDS, "f5", "studio::Present"),
            (PRESENT, "right", "studio::PresentNext"),
            (PRESENT, "space", "studio::PresentNext"),
            (PRESENT, "pagedown", "studio::PresentNext"),
            (PRESENT, "left", "studio::PresentPrevious"),
            (PRESENT, "pageup", "studio::PresentPrevious"),
            (PRESENT, "home", "studio::PresentFirst"),
            (PRESENT, "end", "studio::PresentLast"),
            (PRESENT, "escape", "studio::ExitPresent"),
        ] {
            let keys = gpui::Keystroke::parse(keys).unwrap().unparse();
            assert!(
                bindings.contains(&(context.to_owned(), keys.clone(), action)),
                "{context} {keys} -> {action} missing from {bindings:#?}"
            );
        }
    }
}
