use std::{cell::RefCell, rc::Rc};

use gpui::{ImageFormat, TestAppContext};

use super::*;
use crate::agent::composer::keymap::{
    Backspace, Delete, DeleteToLineEnd, Down, End, Newline, Redo, SelectDown, SelectEnd, SelectUp,
    Undo, Up,
};
use crate::agent::composer::metrics::INPUT_TEXT_SIZE;

const SECONDARY_ENTER: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

const SECONDARY_V: &str = if cfg!(target_os = "macos") {
    "cmd-v"
} else {
    "ctrl-v"
};

fn window(cx: &mut TestAppContext) -> gpui::WindowHandle<ComposerInput> {
    cx.update(keymap::init);
    let handle = cx.add_window(|window, cx| {
        let input = ComposerInput::new("Draft", Theme::dark(), cx);
        window.focus(&input.focus_handle, cx);
        input
    });
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
}

fn with_input(
    cx: &mut TestAppContext,
    test: impl FnOnce(&mut ComposerInput, &mut Window, &mut Context<ComposerInput>),
) {
    let handle = window(cx);
    handle
        .update(cx, |input, window, cx| test(input, window, cx))
        .unwrap();
}

type Events = Rc<RefCell<Vec<ComposerInputEvent>>>;

fn record(cx: &mut TestAppContext, handle: gpui::WindowHandle<ComposerInput>) -> Events {
    let events = Events::default();
    let log = events.clone();
    let entity = handle.entity(cx).unwrap();
    cx.update(|cx| {
        cx.subscribe(&entity, move |_, event: &ComposerInputEvent, _| {
            log.borrow_mut().push(event.clone())
        })
        .detach();
    });
    events
}

/// The events other than cursor and viewport bookkeeping.
fn significant(events: &Events) -> Vec<&'static str> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ComposerInputEvent::Submitted => Some("submitted"),
            ComposerInputEvent::SubmittedNow => Some("submitted-now"),
            ComposerInputEvent::Edited => Some("edited"),
            ComposerInputEvent::CompletionNavigate(-1) => Some("completion-up"),
            ComposerInputEvent::CompletionNavigate(_) => Some("completion-down"),
            ComposerInputEvent::CompletionAccept => Some("completion-accept"),
            ComposerInputEvent::CompletionDismiss => Some("completion-dismiss"),
            ComposerInputEvent::PastedImages(_) => Some("pasted-images"),
            ComposerInputEvent::PastedPaths(_) => Some("pasted-paths"),
            ComposerInputEvent::CursorMoved | ComposerInputEvent::ViewportChanged => None,
        })
        .collect()
}

#[gpui::test]
fn shift_backspace_deletes_text_and_selection(cx: &mut TestAppContext) {
    let handle = window(cx);
    handle
        .update(cx, |input, _, cx| input.set_text("café", cx))
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "shift-backspace");
    handle
        .update(cx, |input, _, cx| {
            assert_eq!(input.text(), "caf");
            input.set_text("hello world", cx);
            input.move_to(6, cx);
            input.extend_selection(11, cx);
        })
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "shift-backspace");
    handle
        .update(cx, |input, _, cx| {
            assert_eq!(input.text(), "hello ");
            input.set_text("", cx);
        })
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "shift-backspace");
    assert_eq!(
        handle
            .read_with(cx, |input, _| input.text().to_owned())
            .unwrap(),
        ""
    );
}

#[gpui::test]
fn enter_submits_and_shift_enter_inserts_a_line_break(cx: &mut TestAppContext) {
    let handle = window(cx);
    let events = record(cx, handle);
    cx.simulate_input(handle.into(), "one");
    cx.simulate_keystrokes(handle.into(), "shift-enter");
    cx.simulate_input(handle.into(), "two");
    assert_eq!(
        handle
            .read_with(cx, |input, _| input.text().to_owned())
            .unwrap(),
        "one\ntwo"
    );
    assert!(!significant(&events).contains(&"submitted"));
    cx.simulate_keystrokes(handle.into(), "enter");
    assert_eq!(significant(&events).last(), Some(&"submitted"));
    // Sending is the owner's call: the input keeps its text until told otherwise.
    assert_eq!(
        handle
            .read_with(cx, |input, _| input.text().to_owned())
            .unwrap(),
        "one\ntwo"
    );
    cx.simulate_keystrokes(handle.into(), SECONDARY_ENTER);
    assert_eq!(significant(&events).last(), Some(&"submitted-now"));
}

#[gpui::test]
fn enter_during_a_composition_belongs_to_the_input_method(cx: &mut TestAppContext) {
    let handle = window(cx);
    let events = record(cx, handle);
    handle
        .update(cx, |input, window, cx| {
            input.replace_and_mark_text_in_range(None, "か", None, window, cx);
            assert!(input.is_composing());
        })
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "enter");
    assert!(!significant(&events).contains(&"submitted"));
    handle
        .update(cx, |input, window, cx| input.unmark_text(window, cx))
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "enter");
    assert_eq!(significant(&events).last(), Some(&"submitted"));
}

#[gpui::test]
fn open_completion_takes_enter_tab_arrows_and_escape(cx: &mut TestAppContext) {
    let handle = window(cx);
    let events = record(cx, handle);
    handle
        .update(cx, |input, _, cx| {
            input.set_text("line one\nline two", cx);
            input.set_completion_controls(true, true, cx);
        })
        .unwrap();
    events.borrow_mut().clear();
    for (keys, expected) in [
        ("enter", "completion-accept"),
        ("tab", "completion-accept"),
        ("up", "completion-up"),
        ("down", "completion-down"),
        ("escape", "completion-dismiss"),
    ] {
        cx.simulate_keystrokes(handle.into(), keys);
        assert_eq!(significant(&events).last(), Some(&expected), "{keys}");
    }
    assert!(!significant(&events).contains(&"submitted"));
    // Without a selected row the arrows move the caret and Enter sends.
    handle
        .update(cx, |input, _, cx| {
            input.set_completion_controls(true, false, cx)
        })
        .unwrap();
    events.borrow_mut().clear();
    cx.simulate_keystrokes(handle.into(), "up");
    cx.simulate_keystrokes(handle.into(), "enter");
    assert_eq!(significant(&events), ["submitted"]);
    // A closed completion lets Escape through.
    handle
        .update(cx, |input, _, cx| {
            input.set_completion_controls(false, false, cx)
        })
        .unwrap();
    events.borrow_mut().clear();
    cx.simulate_keystrokes(handle.into(), "escape");
    assert!(significant(&events).is_empty());
}

#[gpui::test]
fn paste_inserts_text_and_hands_images_to_the_owner(cx: &mut TestAppContext) {
    let handle = window(cx);
    let events = record(cx, handle);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("pasted".into()));
    cx.simulate_keystrokes(handle.into(), SECONDARY_V);
    assert_eq!(
        handle
            .read_with(cx, |input, _| input.text().to_owned())
            .unwrap(),
        "pasted"
    );
    let image = gpui::Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]);
    cx.write_to_clipboard(gpui::ClipboardItem::new_image(&image));
    events.borrow_mut().clear();
    cx.simulate_keystrokes(handle.into(), SECONDARY_V);
    assert_eq!(significant(&events), ["pasted-images"]);
    assert_eq!(
        handle
            .read_with(cx, |input, _| input.text().to_owned())
            .unwrap(),
        "pasted",
        "an image is not inserted as text"
    );
}

#[gpui::test]
fn crlf_navigation_deletion_and_newlines_keep_the_pair_intact(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        let first = "plain café";
        let raw = format!("{first}\r\nnext");
        input.set_text(&raw, cx);
        input.move_to(0, cx);
        input.end(&End, window, cx);
        assert_eq!(input.cursor_offset(), first.len());
        input.layout_text(px(600.0), &window.text_style(), window);
        assert_eq!(
            input.caret_for_point(point(px(590.0), px(1.0))).0,
            first.len()
        );
        assert_eq!(
            input.selection_unit(PressIntent::Line, 0),
            0..first.len() + 2
        );
        input.move_to(0, cx);
        input.select_end(&SelectEnd, window, cx);
        assert_eq!(input.selected_range, 0..first.len());
        input.move_to(0, cx);
        input.end(&End, window, cx);
        input.backspace(&Backspace, window, cx);
        let previous = first.grapheme_indices(true).next_back().unwrap().0;
        assert_eq!(input.text(), format!("{}\r\nnext", &first[..previous]));
        input.undo(&Undo, window, cx);
        input.move_to(first.len(), cx);
        input.delete(&Delete, window, cx);
        assert_eq!(input.text(), format!("{first}next"));
        input.undo(&Undo, window, cx);
        input.move_to(first.len() + 2, cx);
        input.backspace(&Backspace, window, cx);
        assert_eq!(input.text(), format!("{first}next"));
        input.undo(&Undo, window, cx);
        input.move_to(0, cx);
        input.end(&End, window, cx);
        input.newline(&Newline, window, cx);
        assert_eq!(input.text(), format!("{first}\r\n\r\nnext"));
        input.undo(&Undo, window, cx);
        input.move_to(2, cx);
        input.delete_to_line_end(&DeleteToLineEnd, window, cx);
        assert_eq!(input.text(), format!("{}\r\nnext", &first[..2]));
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), raw);

        input.set_text("\r\nnext", cx);
        input.move_to(0, cx);
        input.end(&End, window, cx);
        assert_eq!(input.cursor_offset(), 0);
        input.backspace(&Backspace, window, cx);
        assert_eq!(input.text(), "\r\nnext");
        input.delete(&Delete, window, cx);
        assert_eq!(input.text(), "next");
    });
}

#[gpui::test]
fn soft_wrap_carets_preserve_pointer_keyboard_and_ime_affinity(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.set_text(
            "Café and emoji 🦀 wrap across several visual rows with enough text to keep moving downward.",
            cx,
        );
        input.layout_text(px(140.0), &window.text_style(), window);
        let line = &input.last_lines[0];
        assert!(line.wrap_boundaries().len() >= 2);
        let boundary = line.wrap_boundaries()[0];
        let offset = line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index;
        let upstream = input
            .point_for_index_with_affinity(offset, CaretAffinity::Upstream)
            .unwrap();
        let downstream = input.point_for_index(offset).unwrap();
        assert_eq!(upstream.y, px(0.0));
        assert_eq!(downstream, point(px(0.0), input.line_height));
        assert!(upstream.x > downstream.x);
        assert_eq!(
            input.caret_for_point(upstream + point(px(20.0), px(1.0))),
            (offset, CaretAffinity::Upstream)
        );
        assert_eq!(
            input.caret_for_point(downstream + point(px(0.0), px(1.0))),
            (offset, CaretAffinity::Downstream)
        );
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(140.0), px(200.0)));
        input.last_bounds = Some(bounds);
        for (local, affinity) in [
            (upstream, CaretAffinity::Upstream),
            (downstream, CaretAffinity::Downstream),
        ] {
            input.on_mouse_down(
                &MouseDownEvent {
                    button: MouseButton::Left,
                    position: bounds.origin + local + point(px(0.0), px(1.0)),
                    click_count: 1,
                    ..Default::default()
                },
                window,
                cx,
            );
            assert_eq!(input.cursor_offset(), offset);
            assert_eq!(input.caret_affinity, affinity);
            assert_eq!(input.cursor_point(), Some(local));
            let utf16 = input.offset_to_utf16(offset);
            let ime = input
                .bounds_for_range(utf16..utf16, bounds, window, cx)
                .unwrap();
            assert_eq!(ime.origin, bounds.origin + local);
            input.replace_text_in_range(None, "x", window, cx);
            input.undo(&Undo, window, cx);
            input.layout_text(px(140.0), &window.text_style(), window);
            assert_eq!(input.cursor_offset(), offset);
            assert_eq!(input.caret_affinity, affinity);
            assert_eq!(input.cursor_point(), Some(local));
        }
        input.move_to(0, cx);
        input.down(&Down, window, cx);
        assert_eq!(input.cursor_point(), Some(downstream));
        input.down(&Down, window, cx);
        assert_eq!(input.cursor_point().unwrap().y, input.line_height * 2);
        input.up(&Up, window, cx);
        assert_eq!(input.cursor_point(), Some(downstream));
        input.up(&Up, window, cx);
        assert_eq!(input.cursor_offset(), 0);
        input.select_down(&SelectDown, window, cx);
        assert_eq!(input.selected_range, 0..offset);
        assert_eq!(input.cursor_point(), Some(downstream));
        input.select_up(&SelectUp, window, cx);
        assert_eq!(input.selected_range, 0..0);
    });
}

#[gpui::test]
fn vertical_navigation_retains_column_across_short_and_wrapped_rows(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        let long = "abcdefghijklmnopqrstuvwx";
        let raw = format!("{long}\nx\n{long}");
        let last_start = long.len() + 3;
        for width in [1000.0, 140.0] {
            input.set_text(&raw, cx);
            input.layout_text(px(width), &window.text_style(), window);
            input.move_to(7, cx);
            let start = input.cursor_point().unwrap();
            let target = input.point_for_index(last_start + 7).unwrap();
            let rows =
                (f32::from(target.y - start.y) / f32::from(input.line_height)).round() as usize;
            assert!(rows >= 2);
            for _ in 0..rows {
                input.down(&Down, window, cx);
            }
            assert_eq!(
                input.cursor_offset(),
                last_start + 7,
                "column lost at width {width}"
            );
            assert_eq!(input.preferred_column, Some(start.x));
            for _ in 0..rows {
                input.up(&Up, window, cx);
            }
            assert_eq!(input.cursor_offset(), 7);
            for _ in 0..rows {
                input.select_down(&SelectDown, window, cx);
            }
            assert_eq!(input.selected_range, 7..last_start + 7);
            for _ in 0..rows {
                input.select_up(&SelectUp, window, cx);
            }
            assert_eq!(input.selected_range, 7..7);
            input.right(&keymap::Right, window, cx);
            assert_eq!(input.preferred_column, None);
            input.down(&Down, window, cx);
            assert!(input.preferred_column.is_some());
            input.replace_text_in_range(None, "é", window, cx);
            assert_eq!(input.preferred_column, None);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), raw);
        }
    });
}

#[gpui::test]
fn bulk_edits_and_following_typing_are_separate_undo_steps(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.replace_text_in_range(None, "paste", window, cx);
        input.replace_text_in_range(None, "d", window, cx);
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "paste");
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "");
        input.redo(&Redo, window, cx);
        assert_eq!(input.text(), "paste");
        // A completion is one step that typing after it does not join.
        input.set_text("/co", cx);
        input.replace_range_as_step(0..3, "/compact ", cx);
        input.replace_text_in_range(None, "x", window, cx);
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "/compact ");
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "/co");
    });
}

#[gpui::test]
fn explicit_navigation_breaks_undo_runs_but_backspace_stays_coalesced(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.replace_text_in_range(None, "a", window, cx);
        input.move_to(0, cx);
        input.move_to(1, cx);
        input.replace_text_in_range(None, "b", window, cx);
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "a");
        input.set_text("abcd", cx);
        input.backspace(&Backspace, window, cx);
        input.backspace(&Backspace, window, cx);
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), "abcd");
    });
}

#[gpui::test]
fn ime_uses_replacement_relative_utf16_and_undoes_a_whole_composition(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        let before = "ab\n😀x";
        input.set_text(before, cx);
        let start = before.len() - 1;
        input.selected_range = start..before.len();
        input.selection_reversed = true;
        input.replace_and_mark_text_in_range(None, "あいう", Some(1..2), window, cx);
        assert_eq!(input.selected_range, start + 3..start + 6);
        assert!(!input.selection_reversed);
        assert_eq!(&input.text()[input.selected_range.clone()], "い");
        input.layout_text(px(320.0), &window.text_style(), window);
        assert!(!input.needs_measure);
        assert!(
            input.last_runs.iter().any(|run| run.underline.is_some()),
            "the composition is underlined"
        );
        input.unmark_text(window, cx);
        assert!(input.needs_measure);
        input.undo(&Undo, window, cx);
        assert_eq!(input.text(), before);
        assert_eq!(input.selected_range, start..before.len());
        assert!(input.selection_reversed);
    });
}

#[gpui::test]
fn utf16_ranges_count_surrogate_pairs(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.set_text("a😀b", cx);
        // The emoji is 4 UTF-8 bytes and 2 UTF-16 units.
        assert_eq!(input.range_to_utf16(&(1..5)), 1..3);
        assert_eq!(input.range_from_utf16(&(1..3)), 1..5);
        let mut actual = None;
        let text = input.text_for_range(1..3, &mut actual, window, cx);
        assert_eq!(text.as_deref(), Some("😀"));
        assert_eq!(actual, Some(1..3));
        // An offset past the end clamps instead of panicking.
        input.replace_text_in_range(Some(0..99), "z", window, cx);
        assert_eq!(input.text(), "z");
    });
}

#[gpui::test]
fn layout_cache_reuses_resize_frames_and_invalidates_on_text_changes(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.layout_rebuilds = 0;
        let mut style = window.text_style();
        style.font_size = px(INPUT_TEXT_SIZE).into();
        input.set_text(
            "A wrapped draft with enough text to measure.\n".repeat(100),
            cx,
        );
        input.layout_text(px(400.0), &style, window);
        assert_eq!(input.layout_rebuilds, 1);
        let height = input.content_height;
        for frame in 0..120 {
            input.viewport_height = 40.0 + frame as f32;
            input.scroll_top = frame as f32;
            input.selected_range = 2..8;
            assert_eq!(input.layout_text(px(400.0), &style, window), height);
        }
        assert_eq!(
            input.layout_rebuilds, 1,
            "resize, scroll and selection reuse shaping"
        );
        input.set_text("Edited draft", cx);
        input.layout_text(px(400.0), &style, window);
        assert_eq!(input.layout_rebuilds, 2);
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 3, "a width change rewraps");
        style.font_size = px(18.0).into();
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 4);
        input.marked_range = Some(0..2);
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 5, "marking repaints the decoration");
        input.unmark_text(window, cx);
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 6, "unmarking invalidates too");
        input.set_text("", cx);
        input.layout_text(px(200.0), &style, window);
        input.set_placeholder("New placeholder", cx);
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 8);
        style.color = gpui::rgb(0xff0000).into();
        input.layout_text(px(200.0), &style, window);
        assert_eq!(input.layout_rebuilds, 9);
    });
}

#[gpui::test]
fn placeholder_shows_only_while_empty_and_owns_no_text(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.layout_text(px(300.0), &window.text_style(), window);
        assert!(input.display_is_placeholder);
        assert_eq!(input.text(), "");
        assert_eq!(
            input.caret_for_point(point(px(50.0), px(5.0))),
            (0, CaretAffinity::Downstream)
        );
        input.set_text("x", cx);
        input.layout_text(px(300.0), &window.text_style(), window);
        assert!(!input.display_is_placeholder);
    });
}

#[gpui::test]
fn the_composer_theme_changes_how_the_input_shapes(cx: &mut TestAppContext) {
    with_input(cx, |input, window, cx| {
        input.layout_text(px(300.0), &window.text_style(), window);
        let rebuilds = input.layout_rebuilds;
        input.set_theme(Theme::light(), cx);
        input.layout_text(px(300.0), &window.text_style(), window);
        assert_eq!(input.layout_rebuilds, rebuilds + 1);
        assert_eq!(
            input.last_runs[0].color,
            gpui::Hsla::from(Theme::light().colors.text_3)
        );
    });
}
