//! Kit components the `kit` page's main cards do not cover: the button matrix, text input and
//! number field states, radio cards, the colour picker, circular progress, nested cards and
//! popovers, the tooltip with its key chips and selectable text.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement, Styled,
    Window, div, point, px,
};
use studio_theme::{METRICS, Theme};
use studio_ui::kit::{
    Button, ButtonSize, Card, CircularProgress, ColorPicker, ColorPickerSnapshot, Emphasis,
    FormField, Icon, NumberField, PanelSection, Popover, RadioCard, RadioCards, StatusDot,
    TextInput, TextInputState, Tone, Tooltip, color_swatch, muted, selectable,
};

use crate::Gallery;

/// The entities the extras draw: text inputs in their states and the number field's focus.
#[derive(Default)]
pub struct KitExtras {
    inputs: Option<Inputs>,
}

struct Inputs {
    filled: Entity<TextInputState>,
    empty: Entity<TextInputState>,
    disabled: Entity<TextInputState>,
    multiline: Entity<TextInputState>,
    invalid: Entity<TextInputState>,
    number_focus: FocusHandle,
}

impl Gallery {
    /// The extra kit components, for the `kit` page and the interactive gallery.
    pub(crate) fn render_kit_extras(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let inputs = self.extras.inputs.get_or_insert_with(|| {
            let single =
                |text: &str, placeholder: &str, window: &mut Window, cx: &mut gpui::App| {
                    let (text, placeholder) = (text.to_owned(), placeholder.to_owned());
                    cx.new(|cx| {
                        let mut input = TextInputState::single_line(window, cx);
                        input.set_placeholder(placeholder);
                        input.set_text(text, cx);
                        input
                    })
                };
            let multiline = cx.new(|cx| {
                let mut input = TextInputState::multi_line(window, cx);
                input.set_text("Welcome to the launch.\nThis is the second line.", cx);
                input
            });
            let disabled = single("Read only", "", window, cx);
            disabled.update(cx, |input, cx| input.set_disabled(true, cx));
            Inputs {
                filled: single("launch-video", "", window, cx),
                empty: single("", "Project name", window, cx),
                disabled,
                multiline,
                invalid: single("0", "", window, cx),
                number_focus: cx.focus_handle(),
            }
        });
        let theme = *theme;
        let colors = theme.colors;
        let column = || div().flex().flex_col().gap(px(METRICS.space_3));
        let wrap = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(METRICS.space_3))
        };

        // Buttons: 4 emphases x 2 sizes, each at rest, pressed, disabled and with icons.
        let mut matrix = column();
        for (emphasis, name) in [
            (Emphasis::Ghost, "Ghost"),
            (Emphasis::Neutral, "Neutral"),
            (Emphasis::Primary, "Primary"),
            (Emphasis::Danger, "Danger"),
        ] {
            for (size, size_name) in [(ButtonSize::Md, "md"), (ButtonSize::Sm, "sm")] {
                let id = |part: &str| format!("matrix-{name}-{size_name}-{part}");
                let button = |part: &str, label: &str| {
                    Button::new(&theme, id(part))
                        .label(label.to_owned())
                        .emphasis(emphasis)
                        .size(size)
                };
                matrix = matrix.child(
                    wrap()
                        .child(
                            div()
                                .w(px(METRICS.layout.sidebar_w / 2.))
                                .child(muted(&theme, format!("{name} {size_name}"))),
                        )
                        .child(button("rest", "Rest"))
                        .child(button("pressed", "Pressed").pressed(true))
                        .child(button("disabled", "Disabled").disabled(true))
                        .child(
                            button("icon", "Build")
                                .left_icon(Icon::Hammer.path())
                                .right_icon(Icon::ChevronDown.path()),
                        ),
                );
            }
        }
        let buttons = PanelSection::new(&theme, "Button matrix")
            .description("4 emphases x 2 sizes: rest, pressed, disabled, with icons")
            .child(matrix);

        // Text inputs and the number field.
        let input_row = |title: &str, input: AnyElement| {
            column()
                .w(px(METRICS.layout.sidebar_w))
                .child(muted(&theme, title.to_owned()))
                .child(input)
        };
        let controls = PanelSection::new(&theme, "TextInput, NumberField")
            .description("empty, filled, disabled, multiline, with an error; number field")
            .child(input_row(
                "Empty",
                TextInput::new(&theme, "extras-empty", &inputs.empty).into_any_element(),
            ))
            .child(input_row(
                "Filled",
                TextInput::new(&theme, "extras-filled", &inputs.filled).into_any_element(),
            ))
            .child(input_row(
                "Disabled",
                TextInput::new(&theme, "extras-disabled", &inputs.disabled).into_any_element(),
            ))
            .child(input_row(
                "Multiline",
                TextInput::new(&theme, "extras-multiline", &inputs.multiline)
                    .height(px(METRICS.size.input_h * 2.))
                    .into_any_element(),
            ))
            .child(
                FormField::stacked(
                    &theme,
                    "Frame rate",
                    TextInput::new(&theme, "extras-invalid", &inputs.invalid),
                )
                .error("Must be above 0"),
            )
            .child(
                wrap()
                    .child(NumberField::new(&theme, "1920").width(px(96.)))
                    .child(NumberField::new(&theme, "0.8").unit("x").width(px(96.)))
                    .child(
                        NumberField::new(&theme, "48")
                            .unit("px")
                            .focus(&inputs.number_focus, true)
                            .width(px(96.)),
                    ),
            );

        // Radio cards, the swatch and the colour picker.
        let choices = PanelSection::new(&theme, "RadioCards, colour")
            .description("one selected, one disabled; swatch, filled and empty; the picker")
            .child(RadioCards::new(
                &theme,
                "extras-radio",
                vec![
                    RadioCard::new("Blank", Some("An empty scene")),
                    RadioCard::new("Slide deck", Some("Stops and speaker notes")),
                    RadioCard::new("Template", Some("Not available yet")).disabled(true),
                ],
                Some(1),
            ))
            .child(
                wrap()
                    .child(color_swatch(
                        &theme,
                        "extras-swatch",
                        Some(colors.accent),
                        false,
                        std::rc::Rc::new(|_, _| {}),
                    ))
                    .child(color_swatch(
                        &theme,
                        "extras-swatch-empty",
                        None,
                        true,
                        std::rc::Rc::new(|_, _| {}),
                    )),
            )
            .child(div().relative().h(px(260.)).child(ColorPicker::new(
                &theme,
                ColorPickerSnapshot::new(point(px(0.), px(0.)), [255, 85, 51]),
            )));

        // Feedback: the info status dot and circular progress.
        let feedback = PanelSection::new(&theme, "StatusDot (info), CircularProgress")
            .description("35 % with its label, 100 %, indeterminate, small")
            .child(
                wrap()
                    .child(StatusDot::new(Tone::Info))
                    .child(
                        CircularProgress::new(&theme, px(40.), px(4.))
                            .progress(0.35)
                            .label(px(10.)),
                    )
                    .child(CircularProgress::new(&theme, px(40.), px(4.)).progress(1.))
                    .child(CircularProgress::new(&theme, px(40.), px(4.)).indeterminate())
                    .child(CircularProgress::new(&theme, px(16.), px(2.)).progress(0.6)),
            );

        // Surfaces: a nested card, a popover and the stacked and inline fields.
        let surfaces = PanelSection::new(&theme, "Card nested, Popover, Field")
            .child(
                Card::raised(&theme).p(px(METRICS.space_3)).child(
                    Card::nested(&theme)
                        .p(px(METRICS.space_3))
                        .child(muted(&theme, "A nested card on a raised card".to_owned())),
                ),
            )
            .child(
                // The popover positions itself absolutely, so it gets a box to sit in.
                div().relative().h(px(METRICS.size.control_h * 2.)).child(
                    Popover::new(&theme)
                        .p(px(METRICS.space_3))
                        .child(muted(&theme, "A popover".to_owned())),
                ),
            );

        // Text and keys: the tooltip's content with its key chips, and selectable text.
        let tooltip = Tooltip::new(&theme, "Play or pause")
            .keys(["Space"])
            .view(cx);
        let text = PanelSection::new(&theme, "Tooltip with keys, selectable text")
            .description("the tooltip shows on hover; here its content is drawn in place")
            .child(div().child(tooltip))
            .child(
                div()
                    .text_color(colors.text_1)
                    .child(selectable("Drag to select this text, then press Ctrl+C.")),
            );

        div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(METRICS.space_6))
            .child(column().w(px(520.)).child(buttons))
            .child(
                column()
                    .w(px(METRICS.layout.sidebar_w + 40.))
                    .child(controls)
                    .child(feedback),
            )
            .child(
                column()
                    .w(px(METRICS.layout.sidebar_w + 40.))
                    .child(choices)
                    .child(surfaces)
                    .child(text),
            )
    }
}
