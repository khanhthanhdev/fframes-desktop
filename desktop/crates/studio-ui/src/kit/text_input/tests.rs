use super::*;

// -- Word navigation ------------------------------------------------------

/// `alt-left` skips the run of separators first, then the word, which is
/// what makes it land on the *start* of the previous word rather than on
/// the space in front of it.
#[test]
fn alt_left_lands_on_the_start_of_the_previous_word() {
    let text = "the quick brown fox";
    assert_eq!(word_start_before(text, text.len()), 16); // "fox"
    assert_eq!(word_start_before(text, 16), 10); // "brown"
    assert_eq!(word_start_before(text, 12), 10); // mid-word -> its start
    assert_eq!(word_start_before(text, 0), 0);
}

#[test]
fn alt_right_lands_on_the_end_of_the_next_word() {
    let text = "the quick brown fox";
    assert_eq!(word_end_after(text, 0), 3);
    assert_eq!(word_end_after(text, 3), 9);
    assert_eq!(word_end_after(text, 5), 9); // mid-word -> its end
    assert_eq!(word_end_after(text, text.len()), text.len());
}

/// Punctuation is a separator, not a word character, so `{target_name}`
/// walks in three steps -- which is the behaviour the settings window's
/// project-name template needs.
#[test]
fn punctuation_separates_words() {
    let text = "{target_name} ({date})";
    assert_eq!(word_end_after(text, 0), 12); // "target_name"
    assert_eq!(word_start_before(text, 12), 1);
    assert_eq!(word_end_after(text, 12), 20); // "date"
}

#[test]
fn word_navigation_is_utf8_safe() {
    let text = "héllo wörld";
    assert_eq!(word_end_after(text, 0), "héllo".len());
    assert_eq!(word_start_before(text, text.len()), "héllo ".len());
    // Never lands inside a multi-byte scalar.
    for offset in 0..=text.len() {
        if !text.is_char_boundary(offset) {
            continue;
        }
        assert!(text.is_char_boundary(word_start_before(text, offset)));
        assert!(text.is_char_boundary(word_end_after(text, offset)));
    }
}

// -- Double- and triple-click ---------------------------------------------

#[test]
fn double_click_selects_the_word_under_the_caret() {
    let text = "the quick brown fox";
    assert_eq!(word_at(text, 12), 10..15); // inside "brown"
    assert_eq!(word_at(text, 10), 10..15); // on its first character
    assert_eq!(word_at(text, 15), 15..16); // on the space after it
    // At the very end, the last word rather than an empty range.
    assert_eq!(word_at(text, text.len()), 16..19);
    assert_eq!(word_at("", 0), 0..0);
}

#[test]
fn triple_click_selects_the_paragraph() {
    let text = "first line\nsecond line\nthird";
    assert_eq!(paragraph_start(text, 0), 0);
    assert_eq!(paragraph_end(text, 0), 10);
    assert_eq!(paragraph_start(text, 15), 11);
    assert_eq!(paragraph_end(text, 15), 22);
    // The newline itself belongs to the line it terminates.
    assert_eq!(paragraph_start(text, 10), 0);
    assert_eq!(paragraph_start(text, 11), 11);
    assert_eq!(paragraph_end(text, 23), text.len());
}

/// A field with no newlines has exactly one paragraph, so `cmd-left` and
/// `cmd-right` are Home and End -- which is why the two share an action.
#[test]
fn a_single_line_field_has_one_paragraph() {
    let text = "just one line";
    assert_eq!(paragraph_start(text, 7), 0);
    assert_eq!(paragraph_end(text, 7), text.len());
}

// -- Selection ------------------------------------------------------------

#[test]
fn shift_arrow_extends_and_flips_across_the_anchor() {
    // Extending forwards from a caret at 5.
    let (range, reversed) = extend_selection(5..5, false, 8);
    assert_eq!((range.clone(), reversed), (5..8, false));
    // Extending further forwards.
    let (range, reversed) = extend_selection(range, reversed, 11);
    assert_eq!((range.clone(), reversed), (5..11, false));
    // Back past the anchor: the direction flips and the anchor is kept.
    let (range, reversed) = extend_selection(range, reversed, 2);
    assert_eq!((range.clone(), reversed), (2..5, true));
    // ...and extending again now moves the *start*.
    let (range, reversed) = extend_selection(range, reversed, 0);
    assert_eq!((range, reversed), (0..5, true));
}

#[test]
fn extending_back_onto_the_anchor_empties_the_selection() {
    let (range, reversed) = extend_selection(5..9, false, 5);
    assert_eq!((range, reversed), (5..5, false));
}

// -- Graphemes ------------------------------------------------------------

/// Backspace deletes a user-perceived character. A flag is two scalars and
/// eight bytes; one press has to take all of it.
#[test]
fn backspace_walks_graphemes_not_bytes() {
    let text = "a🇬🇧b";
    let flag_len = "🇬🇧".len();
    assert_eq!(previous_grapheme(text, text.len()), 1 + flag_len);
    assert_eq!(previous_grapheme(text, 1 + flag_len), 1);
    assert_eq!(next_grapheme(text, 1), 1 + flag_len);
    assert_eq!(next_grapheme(text, text.len()), text.len());
}

#[test]
fn a_combining_accent_is_one_grapheme() {
    let text = "e\u{301}"; // e + combining acute
    assert_eq!(previous_grapheme(text, text.len()), 0);
}

// -- utf-16, the input protocol's unit ------------------------------------

#[test]
fn utf16_offsets_round_trip_through_astral_characters() {
    let text = "a😀b"; // 'a' + 4-byte emoji (2 utf-16 units) + 'b'
    assert_eq!(offset_to_utf16(text, 0), 0);
    assert_eq!(offset_to_utf16(text, 1), 1);
    assert_eq!(offset_to_utf16(text, 5), 3);
    assert_eq!(offset_to_utf16(text, 6), 4);
    assert_eq!(offset_from_utf16(text, 0), 0);
    assert_eq!(offset_from_utf16(text, 1), 1);
    assert_eq!(offset_from_utf16(text, 3), 5);
    assert_eq!(offset_from_utf16(text, 4), 6);
}

// -- Single-line flattening ------------------------------------------------

#[test]
fn a_pasted_newline_becomes_a_space_in_a_single_line_field() {
    assert_eq!(flatten_single_line("one\ntwo"), "one two");
    assert_eq!(flatten_single_line("one\r\ntwo"), "one  two");
    assert_eq!(flatten_single_line("no breaks"), "no breaks");
}

// -- Undo grouping ---------------------------------------------------------

#[test]
fn typed_characters_coalesce_into_one_undo_step() {
    assert!(!undo_coalesces(None, UndoGroup::Typing));
    assert!(undo_coalesces(Some(UndoGroup::Typing), UndoGroup::Typing));
    assert!(undo_coalesces(
        Some(UndoGroup::Deleting),
        UndoGroup::Deleting
    ));
    // Typing then deleting is two steps, and back again is a third.
    assert!(!undo_coalesces(
        Some(UndoGroup::Typing),
        UndoGroup::Deleting
    ));
    assert!(!undo_coalesces(
        Some(UndoGroup::Deleting),
        UndoGroup::Typing
    ));
    // A paste is always its own step, even after another paste.
    assert!(!undo_coalesces(
        Some(UndoGroup::Discrete),
        UndoGroup::Discrete
    ));
}

// -- Caret maths across wrapped rows ---------------------------------------

/// A stand-in text system: every row holds exactly `per_row` characters and
/// every character is `advance` wide. That is enough to exercise the row
/// table, which is the only part of the wrapping that is ours.
fn wrapped(text: &str, per_row: usize, line_height: f32) -> (Vec<usize>, usize, Vec<usize>) {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(text.len()))
        .collect();
    let row_count = boundaries.len().div_ceil(per_row).max(1);
    let y_of = |index: usize| {
        // Mirrors gpui: an index that lands on a boundary reports the row
        // it *ends*, so the row index is `ceil(index / per_row) - 1`.
        let row = if index == 0 {
            0
        } else {
            index.div_ceil(per_row).saturating_sub(1)
        };
        row as f32 * line_height
    };
    let starts = row_starts(&boundaries, row_count, line_height, y_of);
    (boundaries, row_count, starts)
}

#[test]
fn row_starts_follow_the_wrap_boundaries() {
    // 12 ASCII characters, 4 per row -> rows start at 0, 4, 8.
    let (_, row_count, starts) = wrapped("abcdefghijkl", 4, 20.);
    assert_eq!(row_count, 4); // 13 boundaries / 4, rounded up
    assert_eq!(&starts[..3], &[0, 4, 8]);
}

#[test]
fn a_paragraph_that_does_not_wrap_has_one_row() {
    let (_, row_count, starts) = wrapped("short", 40, 20.);
    assert_eq!(row_count, 1);
    assert_eq!(starts, vec![0]);
}

#[test]
fn row_starts_of_an_empty_paragraph_is_just_zero() {
    let (_, row_count, starts) = wrapped("", 4, 20.);
    assert_eq!(row_count, 1);
    assert_eq!(starts, vec![0]);
}

fn layout_of(rows: &[Range<usize>], widths: &[f32]) -> InputLayout {
    InputLayout {
        lines: SmallVec::new(),
        paragraph_starts: vec![0],
        paragraph_rows: vec![0],
        rows: rows
            .iter()
            .enumerate()
            .map(|(index, range)| Row {
                range: range.clone(),
                paragraph: 0,
                local_start: range.start,
                width: px(widths[index]),
            })
            .collect(),
        key: LayoutKey {
            text: SharedString::default(),
            wrap_width: None,
            font_size: px(12.),
            line_height: px(20.),
            weight: FontWeight::NORMAL,
            marked: None,
            color: gpui::transparent_black(),
        },
    }
}

/// The caret at a soft-wrap boundary belongs to the row it *starts*, which
/// is where it has to be after moving right onto it -- gpui's own
/// `position_for_index` would put it at the end of the previous row.
#[test]
fn a_caret_on_a_wrap_boundary_takes_the_later_row() {
    let layout = layout_of(&[0..4, 4..8, 8..12], &[40., 40., 40.]);
    assert_eq!(layout.row_for_offset(0), 0);
    assert_eq!(layout.row_for_offset(3), 0);
    assert_eq!(layout.row_for_offset(4), 1);
    assert_eq!(layout.row_for_offset(7), 1);
    assert_eq!(layout.row_for_offset(8), 2);
    assert_eq!(layout.row_for_offset(12), 2);
    // Past the end still resolves to the last row rather than panicking.
    assert_eq!(layout.row_for_offset(99), 2);
}

#[test]
fn the_caret_row_is_the_start_of_the_row_it_opens() {
    let layout = layout_of(&[0..4, 4..8], &[40., 40.]);
    // Offset 4 opens row 1, so its x inside that row is zero.
    assert_eq!(layout.x_for_offset(4, px(20.)), px(0.));
}

// -- Key bindings ------------------------------------------------------------

fn bound(macos: bool) -> Vec<(String, &'static str)> {
    key_bindings(macos)
        .iter()
        .map(|binding| {
            let keys = binding
                .keystrokes()
                .iter()
                .map(|keystroke| keystroke.inner().unparse())
                .collect::<Vec<_>>()
                .join(" ");
            (keys, binding.action().name())
        })
        .collect()
}

fn action_for(bindings: &[(String, &'static str)], keys: &str) -> Option<&'static str> {
    bindings
        .iter()
        .find(|(bound, _)| *bound == canonical(keys))
        .map(|(_, action)| *action)
}

/// `keys` as gpui spells a parsed keystroke on this host (the platform key is `cmd`, `win`
/// or `super` depending on where the test runs).
fn canonical(keys: &str) -> String {
    gpui::Keystroke::parse(keys).unwrap().unparse()
}

#[test]
fn windows_and_linux_use_ctrl_for_words_and_home_end_for_lines() {
    let bindings = bound(false);
    for (keys, action) in [
        ("ctrl-left", "studio_text_input::MoveWordLeft"),
        ("ctrl-right", "studio_text_input::MoveWordRight"),
        ("ctrl-shift-left", "studio_text_input::SelectWordLeft"),
        ("ctrl-backspace", "studio_text_input::DeleteWordLeft"),
        ("home", "studio_text_input::MoveToLineStart"),
        ("end", "studio_text_input::MoveToLineEnd"),
        ("ctrl-home", "studio_text_input::MoveToStart"),
        ("ctrl-a", "studio_text_input::SelectAll"),
        ("ctrl-c", "studio_text_input::Copy"),
        ("ctrl-v", "studio_text_input::Paste"),
        ("ctrl-z", "studio_text_input::Undo"),
        ("ctrl-y", "studio_text_input::Redo"),
        ("enter", "studio_text_input::Confirm"),
        ("escape", "studio_text_input::Cancel"),
    ] {
        assert_eq!(action_for(&bindings, keys), Some(action), "{keys}");
    }
    let platform_or_alt =
        [canonical("cmd-a"), canonical("alt-a")].map(|keys| keys.replace("-a", "-"));
    assert!(
        bindings.iter().all(|(keys, _)| platform_or_alt
            .iter()
            .all(|modifier| !keys.contains(modifier.as_str()))),
        "{bindings:?}"
    );
}

#[test]
fn macos_uses_alt_for_words_and_cmd_for_lines() {
    let bindings = bound(true);
    for (keys, action) in [
        ("alt-left", "studio_text_input::MoveWordLeft"),
        ("cmd-left", "studio_text_input::MoveToLineStart"),
        ("cmd-right", "studio_text_input::MoveToLineEnd"),
        ("cmd-a", "studio_text_input::SelectAll"),
        ("cmd-z", "studio_text_input::Undo"),
        ("cmd-shift-z", "studio_text_input::Redo"),
    ] {
        assert_eq!(action_for(&bindings, keys), Some(action), "{keys}");
    }
    assert_eq!(action_for(&bindings, "ctrl-left"), None);
}

#[test]
fn no_binding_is_a_bare_printable_key() {
    for macos in [false, true] {
        for (keys, action) in bound(macos) {
            let bare = !keys.contains('-') && keys.chars().count() == 1;
            assert!(!bare, "{keys} -> {action} would swallow typed text");
        }
    }
}

// -- In a window ----------------------------------------------------------

mod in_a_window {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    struct Form {
        first: Entity<TextInputState>,
        second: Entity<TextInputState>,
    }

    impl gpui::Render for Form {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            div()
                .flex()
                .flex_col()
                .w(px(240.))
                .child(TextInput::new(&theme, "first", &self.first))
                .child(TextInput::new(&theme, "second", &self.second))
        }
    }

    type Events = Rc<RefCell<Vec<TextInputEvent>>>;

    fn form(cx: &mut TestAppContext) -> (Entity<Form>, Events, &mut VisualTestContext) {
        cx.update(bind_keys);
        let (form, cx) = cx.add_window_view(|window, cx| Form {
            first: cx.new(|cx| TextInputState::single_line(window, cx)),
            second: cx.new(|cx| TextInputState::single_line(window, cx)),
        });
        let events = Events::default();
        let log = events.clone();
        cx.update(|_, cx| {
            let first = form.read(cx).first.clone();
            cx.subscribe(&first, move |_, event: &TextInputEvent, _| {
                log.borrow_mut().push(event.clone())
            })
            .detach();
        });
        cx.run_until_parked();
        (form, events, cx)
    }

    fn focus_first(form: &Entity<Form>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let focus = form.read(cx).first.read(cx).focus_handle();
            window.focus(&focus, cx);
        });
    }

    fn keys_seen(events: &Events) -> Vec<TextInputEvent> {
        events
            .borrow()
            .iter()
            .filter(|event| **event != TextInputEvent::Changed)
            .cloned()
            .collect()
    }

    #[gpui::test]
    fn enter_and_escape_during_a_composition_stay_with_the_ime(cx: &mut TestAppContext) {
        let (form, events, cx) = form(cx);
        focus_first(&form, cx);
        cx.simulate_input("abc");
        cx.update(|window, cx| {
            let first = form.read(cx).first.clone();
            first.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "\u{304b}", None, window, cx)
            });
        });
        let composing = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| form.read(cx).first.read(cx).is_composing())
        };
        assert!(composing(cx));
        cx.simulate_keystrokes("escape");
        assert!(composing(cx), "Escape went to the IME");
        // Return goes to the IME too, which commits the composition with it.
        cx.simulate_keystrokes("enter");
        assert_eq!(keys_seen(&events), []);

        cx.update(|window, cx| {
            let first = form.read(cx).first.clone();
            first.update(cx, |input, cx| input.unmark_text(window, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.simulate_keystrokes("escape");
        assert_eq!(
            keys_seen(&events),
            [TextInputEvent::Confirmed, TextInputEvent::Cancelled]
        );
    }

    #[gpui::test]
    fn tab_reaches_enabled_fields_only(cx: &mut TestAppContext) {
        let (form, _, cx) = form(cx);
        let focused = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                let form = form.read(cx);
                (
                    form.first.read(cx).focus_handle().is_focused(window),
                    form.second.read(cx).focus_handle().is_focused(window),
                )
            })
        };
        cx.update(|_, cx| {
            let first = form.read(cx).first.clone();
            first.update(cx, |input, cx| input.set_disabled(true, cx));
        });
        cx.update(|window, cx| window.focus_next(cx));
        assert_eq!(focused(cx), (false, true));

        cx.update(|_, cx| {
            let first = form.read(cx).first.clone();
            first.update(cx, |input, cx| input.set_disabled(false, cx));
        });
        cx.update(|window, cx| window.focus_prev(cx));
        assert_eq!(focused(cx), (true, false));
    }

    #[gpui::test]
    fn the_painted_text_bounds_are_readable(cx: &mut TestAppContext) {
        let (form, _, cx) = form(cx);
        let bounds = cx.update(|_, cx| form.read(cx).first.read(cx).text_bounds());
        let bounds = bounds.expect("painted");
        assert!(f32::from(bounds.size.width) > 0.);
    }
}
