use std::{cell::RefCell, rc::Rc};

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::*;
use crate::agent::composer::keymap::{self, EnterOutcome};

const SECONDARY_ENTER: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

type Log = Rc<RefCell<Vec<String>>>;

fn describe(event: &ComposerEvent) -> String {
    match event {
        ComposerEvent::Submit(s) => format!("submit:{}:{}", s.text, s.attachments.len()),
        ComposerEvent::SendAtBoundary(s) => format!("boundary:{}:{}", s.text, s.attachments.len()),
        ComposerEvent::Steer(s) => format!("steer:{}:{}", s.text, s.attachments.len()),
        ComposerEvent::SendNow(s) => format!("now:{}:{}", s.text, s.attachments.len()),
        ComposerEvent::Stop => "stop".into(),
        ComposerEvent::AttachRequested(AttachRequest::PickFiles) => "pick-files".into(),
        ComposerEvent::AttachRequested(AttachRequest::PickImages) => "pick-images".into(),
        ComposerEvent::AttachRequested(AttachRequest::Paths(p)) => format!("paths:{}", p.len()),
        ComposerEvent::AttachRequested(AttachRequest::Images(i)) => format!("images:{}", i.len()),
        ComposerEvent::AttachmentRemoved(id) => format!("removed:{}", id.0),
        ComposerEvent::AttachBlocked { kind, reason } => format!("blocked:{kind:?}:{reason}"),
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<Composer>, &mut VisualTestContext, Log) {
    cx.update(keymap::init);
    let (composer, cx) = cx.add_window_view(|window, cx| {
        let composer = Composer::new("Message", Theme::dark(), cx);
        composer.focus(window, cx);
        composer
    });
    let log = Log::default();
    let sink = log.clone();
    cx.update(|_, cx| {
        cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
            sink.borrow_mut().push(describe(event))
        })
        .detach();
    });
    cx.run_until_parked();
    (composer, cx, log)
}

fn commands() -> Vec<SlashCommand> {
    ["compact", "review", "init", "clear", "cost"]
        .into_iter()
        .map(|name| SlashCommand::new(name, format!("{name} description")))
        .collect()
}

fn text(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    composer.read_with(cx, |composer, cx| composer.text(cx))
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[gpui::test]
fn send_popup_accepts_keyboard_activation_and_escape_restores_the_input(cx: &mut TestAppContext) {
    use crate::kit::test_support::{press, tab};

    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_running(true, cx));
    cx.simulate_input("urgent");
    cx.run_until_parked();
    // The file and image controls precede More ways to send.
    for _ in 0..3 {
        tab(cx);
    }
    press(cx, "enter");
    assert!(composer.read_with(cx, |composer, _| composer.send_menu_open));
    press(cx, "escape");
    assert!(!composer.read_with(cx, |composer, _| composer.send_menu_open));
    assert!(cx.update(|window, cx| composer.focus_handle(cx).is_focused(window)));
    assert_eq!(text(&composer, cx), "urgent");
    assert!(take(&log).is_empty());

    for _ in 0..3 {
        tab(cx);
    }
    press(cx, "enter");
    press(cx, "space");
    assert_eq!(take(&log), ["now:urgent:0"]);
    assert_eq!(text(&composer, cx), "");
    assert!(cx.update(|window, cx| composer.focus_handle(cx).is_focused(window)));
}

#[gpui::test]
fn an_empty_send_popup_cannot_submit_but_can_be_dismissed(cx: &mut TestAppContext) {
    use crate::kit::test_support::{press, tab};

    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_running(true, cx));
    cx.run_until_parked();
    for _ in 0..3 {
        tab(cx);
    }
    press(cx, "enter");
    assert!(composer.read_with(cx, |composer, _| composer.send_menu_open));
    press(cx, "enter");
    assert!(take(&log).is_empty());
    press(cx, "escape");
    assert!(!composer.read_with(cx, |composer, _| composer.send_menu_open));
    assert!(cx.update(|window, cx| composer.focus_handle(cx).is_focused(window)));
}

#[gpui::test]
fn enter_sends_and_shift_enter_inserts_a_line_break(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    cx.simulate_input("one");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("two");
    assert_eq!(text(&composer, cx), "one\ntwo");
    assert!(take(&log).is_empty(), "a line break sends nothing");
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&log), ["submit:one\ntwo:0"]);
    assert_eq!(text(&composer, cx), "", "sending clears the draft");
}

#[gpui::test]
fn an_empty_draft_sends_nothing(cx: &mut TestAppContext) {
    let (_composer, cx, log) = setup(cx);
    cx.simulate_keystrokes("enter");
    cx.simulate_input("   ");
    cx.simulate_keystrokes("enter");
    assert!(take(&log).is_empty());
}

#[gpui::test]
fn a_refused_submission_is_restored_with_its_chips(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    let submitted = Rc::new(RefCell::new(None));
    let slot = submitted.clone();
    cx.update(|_, cx| {
        cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
            if let ComposerEvent::Submit(submission) = event {
                *slot.borrow_mut() = Some(submission.clone());
            }
        })
        .detach();
    });
    let id = composer.update(cx, |composer, cx| {
        composer.add_attachment(AttachmentDraft::file("notes.md"), cx)
    });
    cx.simulate_input("keep me");
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&log), ["submit:keep me:1"]);
    composer.read_with(cx, |composer, _| assert!(composer.attachments().is_empty()));
    let submission = submitted.borrow_mut().take().unwrap();
    composer.update(cx, |composer, cx| composer.restore(submission, cx));
    assert_eq!(text(&composer, cx), "keep me");
    composer.read_with(cx, |composer, _| {
        let ids: Vec<_> = composer.attachments().iter().map(|a| a.id).collect();
        assert_eq!(ids, [id]);
    });
}

#[gpui::test]
fn slash_menu_filters_as_you_type_and_enter_inserts_the_command(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_commands(commands(), cx));
    cx.simulate_input("/");
    composer.read_with(cx, |composer, _| {
        assert!(composer.slash.is_open());
        assert_eq!(composer.slash.rows().len(), 5);
    });
    cx.simulate_input("co");
    composer.read_with(cx, |composer, _| {
        let names: Vec<_> = composer
            .slash
            .rows()
            .iter()
            .map(|i| composer.commands[*i].name.to_string())
            .collect();
        assert_eq!(names, ["compact", "cost"]);
    });
    cx.simulate_keystrokes("down");
    cx.simulate_keystrokes("enter");
    assert_eq!(text(&composer, cx), "/cost ");
    composer.read_with(cx, |composer, _| assert!(!composer.slash.is_open()));
    assert!(take(&log).is_empty(), "accepting a row does not send");
    // The completion is one undo step.
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    });
    assert_eq!(text(&composer, cx), "/co");
}

#[gpui::test]
fn tab_accepts_and_escape_dismisses_the_slash_menu(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_commands(commands(), cx));
    cx.simulate_input("/com");
    cx.simulate_keystrokes("tab");
    assert_eq!(text(&composer, cx), "/compact ");

    composer.update(cx, |composer, cx| composer.set_text("", cx));
    cx.simulate_input("/re");
    composer.read_with(cx, |composer, _| assert!(composer.slash.is_open()));
    cx.simulate_keystrokes("escape");
    composer.read_with(cx, |composer, _| assert!(!composer.slash.is_open()));
    assert_eq!(text(&composer, cx), "/re");
    // With the menu closed Enter sends the draft as typed.
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&log), ["submit:/re:0"]);
}

#[gpui::test]
fn text_that_is_not_a_command_never_opens_the_menu(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_commands(commands(), cx));
    cx.simulate_input("see /usr/bin");
    composer.read_with(cx, |composer, _| assert!(!composer.slash.is_open()));
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&log), ["submit:see /usr/bin:0"]);
}

#[gpui::test]
fn chips_are_added_and_removed_with_an_event_for_the_owner(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    let (a, b) = composer.update(cx, |composer, cx| {
        (
            composer.add_attachment(AttachmentDraft::file("a.txt"), cx),
            composer.add_attachment(AttachmentDraft::file("b.txt"), cx),
        )
    });
    composer.read_with(cx, |composer, _| {
        assert_eq!(composer.attachments().len(), 2)
    });
    composer.update(cx, |composer, cx| composer.remove_from_strip(a, cx));
    assert_eq!(take(&log), [format!("removed:{}", a.0)]);
    composer.read_with(cx, |composer, _| {
        assert_eq!(composer.attachments().len(), 1);
        assert_eq!(composer.attachments()[0].id, b);
    });
    // Removing it again is not an event.
    composer.update(cx, |composer, cx| composer.remove_from_strip(a, cx));
    assert!(take(&log).is_empty());
    // The owner can drop a chip silently.
    assert!(composer.update(cx, |composer, cx| composer.remove_attachment(b, cx)));
    assert!(take(&log).is_empty());
    // A chip alone is enough to send.
    composer.update(cx, |composer, cx| {
        composer.add_attachment(AttachmentDraft::file("c.txt"), cx);
    });
    cx.simulate_keystrokes("enter");
    assert_eq!(take(&log), ["submit::1"]);
}

#[gpui::test]
fn attach_buttons_and_pastes_ask_the_owner_unless_the_kind_is_disabled(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| {
        composer.request_attach(AttachRequest::PickFiles, cx);
        composer.request_attach(AttachRequest::PickImages, cx);
        composer.on_input_event(&ComposerInputEvent::PastedPaths(vec!["a".into()]), cx);
        composer.on_input_event(&ComposerInputEvent::PastedImages(vec![]), cx);
    });
    assert_eq!(
        take(&log),
        ["pick-files", "pick-images", "paths:1", "images:0"]
    );
    composer.update(cx, |composer, cx| {
        composer.set_attach_disabled(AttachKind::Image, Some("no images".into()), cx);
        composer.on_input_event(&ComposerInputEvent::PastedImages(vec![]), cx);
        composer.on_input_event(&ComposerInputEvent::PastedPaths(vec!["a".into()]), cx);
    });
    assert_eq!(
        take(&log),
        ["blocked:Image:no images", "paths:1"],
        "a disabled kind is refused with its reason, not silently dropped"
    );
    composer.update(cx, |composer, cx| {
        composer.set_attach_disabled(AttachKind::Image, None, cx);
        composer.on_input_event(&ComposerInputEvent::PastedImages(vec![]), cx);
    });
    assert_eq!(take(&log), ["images:0"]);
}

#[gpui::test]
fn switching_a_kind_off_removes_its_chips_and_says_why(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| {
        composer.add_attachment(AttachmentDraft::file("notes.txt"), cx);
        let image = Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, vec![0]));
        composer.add_attachment(AttachmentDraft::image("shot", image), cx);
        composer.set_attach_disabled(AttachKind::Image, Some("no images".into()), cx);
    });
    let left = composer.read_with(cx, |composer, _| {
        composer
            .attachments()
            .iter()
            .map(|chip| chip.draft.label.to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(left, ["notes.txt"]);
    let log = take(&log);
    assert_eq!(log.len(), 2, "{log:?}");
    assert!(log[0].starts_with("removed:"));
    assert_eq!(log[1], "blocked:Image:no images");
}

#[gpui::test]
fn idle_enter_and_the_send_now_chord_both_submit(cx: &mut TestAppContext) {
    let (_composer, cx, log) = setup(cx);
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    cx.simulate_keystrokes(SECONDARY_ENTER);
    assert_eq!(take(&log), ["submit:a:0", "submit:b:0"]);
}

#[gpui::test]
fn running_with_boundary_steering_sends_at_boundary_or_now(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| {
        composer.set_steer_mode(SteerMode::AtBoundary, cx);
        composer.set_running(true, cx);
    });
    cx.simulate_input("later");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("urgent");
    cx.simulate_keystrokes(SECONDARY_ENTER);
    assert_eq!(take(&log), ["boundary:later:0", "now:urgent:0"]);
}

#[gpui::test]
fn running_with_native_steering_steers_or_sends_now(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| {
        composer.set_steer_mode(SteerMode::Native, cx);
        composer.set_running(true, cx);
    });
    cx.simulate_input("nudge");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("stop that");
    cx.simulate_keystrokes(SECONDARY_ENTER);
    assert_eq!(take(&log), ["steer:nudge:0", "now:stop that:0"]);
}

#[gpui::test]
fn running_without_steering_keeps_the_draft_on_enter_and_cancels_on_the_chord(
    cx: &mut TestAppContext,
) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.set_running(true, cx));
    cx.simulate_input("wait");
    cx.simulate_keystrokes("enter");
    assert!(take(&log).is_empty());
    assert_eq!(text(&composer, cx), "wait", "the draft survives");
    cx.simulate_keystrokes(SECONDARY_ENTER);
    assert_eq!(take(&log), ["now:wait:0"]);
}

#[gpui::test]
fn stop_emits_only_while_a_turn_runs(cx: &mut TestAppContext) {
    let (composer, cx, log) = setup(cx);
    composer.update(cx, |composer, cx| composer.stop(cx));
    assert!(take(&log).is_empty(), "no turn, no stop");
    composer.update(cx, |composer, cx| {
        composer.set_running(true, cx);
        composer.stop(cx);
    });
    assert_eq!(take(&log), ["stop"]);
}

#[test]
fn enter_goes_to_a_selected_completion_before_anything_else() {
    assert_eq!(
        keymap::enter_outcome(true, EnterOutcome::Submit),
        EnterOutcome::AcceptCompletion
    );
    assert_eq!(
        keymap::enter_outcome(false, EnterOutcome::Newline),
        EnterOutcome::Newline
    );
    assert!(keymap::escape_dismisses_completion("escape", true));
    assert!(!keymap::escape_dismisses_completion("escape", false));
    assert!(!keymap::escape_dismisses_completion("enter", true));
}

#[test]
fn bindings_bind_enter_shift_enter_and_the_chord_per_platform() {
    for macos in [true, false] {
        let bound: Vec<gpui::Keystroke> = keymap::bindings(macos)
            .iter()
            .flat_map(|binding| binding.keystrokes().to_vec())
            .map(|k| k.inner().clone())
            .collect();
        let chord = if macos { "cmd-enter" } else { "ctrl-enter" };
        for key in ["enter", "shift-enter", "tab", chord] {
            let wanted = gpui::Keystroke::parse(key).unwrap();
            assert!(
                bound
                    .iter()
                    .any(|k| k.key == wanted.key && k.modifiers == wanted.modifiers),
                "{key} unbound (macos {macos})"
            );
        }
        // A bare printable key would stop that letter from being typed.
        assert!(
            bound
                .iter()
                .all(|k| k.modifiers.modified() || k.key.len() > 1),
            "a printable key is bound"
        );
    }
}

/// The composer at a fixed width, with the bounds of its named parts.
mod geometry {
    use std::collections::HashMap;

    use gpui::{Context, IntoElement, Render, Window};

    use super::*;
    use crate::agent::composer::metrics::{
        CONTROLS_GAP_TOP, INPUT_INSET_Y, INPUT_MAX_HEIGHT, INPUT_REST_HEIGHT, SHELL_PAD_X,
    };

    type Bounds = Rc<RefCell<HashMap<String, [f32; 4]>>>;

    struct Host {
        composer: Entity<Composer>,
        width: f32,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).child(self.composer.clone())
        }
    }

    /// The card is the controls row plus its padding and its 1 px border on each side.
    fn card_width(controls: f32) -> f32 {
        controls + 2.0 * SHELL_PAD_X + 2.0
    }

    fn mount(
        width: f32,
        cx: &mut TestAppContext,
    ) -> (Entity<Composer>, &mut VisualTestContext, Bounds) {
        cx.update(keymap::init);
        let bounds = Bounds::default();
        let sink = bounds.clone();
        let (host, cx) = cx.add_window_view(|window, cx| {
            let composer = cx.new(|cx| {
                let mut composer = Composer::new("Message", Theme::dark(), cx);
                composer.set_bounds_sink(Rc::new(move |name, rect, _| {
                    sink.borrow_mut().insert(name.to_owned(), rect);
                }));
                composer
            });
            let focus = composer.focus_handle(cx);
            window.focus(&focus, cx);
            Host { composer, width }
        });
        let composer = host.read_with(cx, |host, _| host.composer.clone());
        redraw(&composer, cx, &bounds);
        (composer, cx, bounds)
    }

    /// Draws twice from a clean record: the narrow sizes follow the measured row from the
    /// next frame.
    fn redraw(composer: &Entity<Composer>, cx: &mut VisualTestContext, bounds: &Bounds) {
        for _ in 0..2 {
            bounds.borrow_mut().clear();
            composer.update(cx, |_, cx| cx.notify());
            cx.run_until_parked();
        }
    }

    fn get(bounds: &Bounds, name: &str) -> Option<[f32; 4]> {
        bounds.borrow().get(name).copied()
    }

    fn side(bounds: &Bounds, name: &str) -> f32 {
        get(bounds, name).unwrap_or_else(|| panic!("{name} is not drawn"))[2]
    }

    #[gpui::test]
    fn the_input_rests_at_56_px_and_grows_to_the_160_px_cap(cx: &mut TestAppContext) {
        let (composer, cx, bounds) = mount(card_width(480.), cx);
        assert_eq!(
            get(&bounds, "composer-input").unwrap()[3],
            INPUT_REST_HEIGHT
        );
        cx.simulate_input("one line");
        redraw(&composer, cx, &bounds);
        assert_eq!(
            get(&bounds, "composer-input").unwrap()[3],
            INPUT_REST_HEIGHT
        );
        for _ in 0..3 {
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("more");
        }
        redraw(&composer, cx, &bounds);
        let four_lines = get(&bounds, "composer-input").unwrap()[3];
        assert!(four_lines > INPUT_REST_HEIGHT, "{four_lines}");
        for _ in 0..20 {
            cx.simulate_keystrokes("shift-enter");
            cx.simulate_input("more");
        }
        redraw(&composer, cx, &bounds);
        let capped = get(&bounds, "composer-input").unwrap()[3];
        // The text area stops at the cap; its insets come on top.
        assert_eq!(capped, INPUT_MAX_HEIGHT + 2.0 * INPUT_INSET_Y);
        // The controls sit under the input, the caret's row still above them.
        let send = get(&bounds, "composer-send").unwrap();
        let input = get(&bounds, "composer-input").unwrap();
        assert!(send[1] >= input[1] + input[3] + CONTROLS_GAP_TOP - 0.5);
    }

    #[gpui::test]
    fn send_is_32_px_and_shrinks_to_28_in_a_narrow_composer(cx: &mut TestAppContext) {
        for (controls, narrow) in [
            (296., true),
            (384., true),
            (385., false),
            (416., false),
            (496., false),
        ] {
            let (_composer, _cx, bounds) = mount(card_width(controls), cx);
            let (send, attach) = if narrow { (28., 24.) } else { (32., 28.) };
            assert_eq!(side(&bounds, "composer-send"), send, "row {controls}");
            assert_eq!(
                side(&bounds, "composer-attach-file"),
                attach,
                "row {controls}"
            );
            assert_eq!(
                side(&bounds, "composer-attach-image"),
                attach,
                "row {controls}"
            );
        }
    }

    #[gpui::test]
    fn every_running_mode_keeps_stop_and_send_where_supported_in_reach(cx: &mut TestAppContext) {
        let width = card_width(296.);
        for (mode, has_send) in [
            (SteerMode::AtBoundary, true),
            (SteerMode::Native, true),
            (SteerMode::None, false),
        ] {
            let (composer, cx, bounds) = mount(width, cx);
            composer.update(cx, |composer, cx| {
                composer.set_steer_mode(mode, cx);
                composer.set_running(true, cx);
                composer.set_model(
                    Some(ModelChoice {
                        label: "A very long model name that cannot possibly fit".into(),
                        on_open: Rc::new(|_, _, _| {}),
                    }),
                    cx,
                );
            });
            redraw(&composer, cx, &bounds);
            let stop = get(&bounds, "composer-stop").expect("Stop shows while running");
            assert_eq!(stop[2], 28., "{mode:?}");
            assert!(
                stop[0] + stop[2] <= width - SHELL_PAD_X + 0.5,
                "{mode:?}: {stop:?}"
            );
            match get(&bounds, "composer-send") {
                Some(send) => {
                    assert!(has_send, "{mode:?} has no send");
                    assert!(send[0] + send[2] <= stop[0], "Send precedes Stop");
                }
                None => assert!(!has_send, "{mode:?} lost its send"),
            }
            // The attachment targets stay on screen beside the truncated model.
            assert!(get(&bounds, "composer-attach-file").unwrap()[0] >= 0.);
        }
    }
}
