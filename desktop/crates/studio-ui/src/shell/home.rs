//! The start screen while no project is open, after Oleafly's project shelf: a dotted
//! surface, a floating dock of the project actions at its left edge, a top bar with the
//! brand, a search over the recent projects and the grid or list switch, and the recent
//! projects as book covers or as table rows. Also the compact recent-projects list the
//! Project tab shows while a project is open.

use std::path::Path;

use gpui::{
    AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, MouseButton,
    ParentElement, StatefulInteractiveElement, Styled, Window, canvas, div, fill, point,
    prelude::FluentBuilder, px, size,
};
use studio_engine::engine::backend::{Command, Recent};
use studio_theme::{AppTheme, METRICS, TextStyle, Theme, text_style};

use super::{Picker, StudioShell};
use crate::kit::{
    Badge, Button, ButtonSize, Emphasis, EmptyState, Icon, IconButton, IconButtonSize, List,
    ListRow, SegmentOption, SegmentedControl, TextInput, TextInputEvent, TextInputState, Tone,
    focusable, on_activate, reveal_actions, with_tooltip,
};

// Oleafly's shelf proportions (`Library.tsx`, `Book.tsx`, `HomeDock.tsx`), used by this
// screen only.

/// The dot grid's pitch and the dots' diameter (Oleafly's `DotPattern` 22 px, radius 1).
const DOT_PITCH: f32 = 22.;
const DOT_SIZE: f32 = 2.;
/// The top bar's height (Oleafly's `h-16`) and the search field's height and corner
/// (`h-11 rounded-2xl`), at most `max-w-[42rem]` wide.
const TOP_BAR_H: f32 = 64.;
const SEARCH_H: f32 = 44.;
const SEARCH_MAX_W: f32 = 672.;
/// The dock's corner (`rounded-2xl`) and inner padding (`p-1.5`).
const DOCK_RADIUS: f32 = 16.;
const DOCK_PAD: f32 = 6.;
/// The content keeps clear of the dock: its inset, its padding and a 32 px button.
const DOCK_CLEARANCE: f32 = 72.;
/// A book: 180 px wide at Oleafly's 8.5 : 11 page, a 10 px spine and a back cover peeking
/// 6 px below the front one.
const BOOK_W: f32 = 180.;
const BOOK_H: f32 = BOOK_W * 11. / 8.5;
const BOOK_SPINE_W: f32 = 10.;
const BOOK_EDGE: f32 = 6.;
/// The shelf's column gap (Oleafly's `gap-x-8`), row gap and widest extent (`max-w-5xl`).
const SHELF_GAP_X: f32 = 32.;
const SHELF_GAP_Y: f32 = 28.;
const SHELF_MAX_W: f32 = 1024.;
/// A list row (a 36 px cover thumbnail with room around it), its thumbnail, and the fixed
/// Status and actions columns.
const LIST_ROW_H: f32 = 56.;
const LIST_THUMB_W: f32 = 28.;
const LIST_THUMB_H: f32 = 36.;
const LIST_STATUS_W: f32 = 112.;
const LIST_ACTIONS_W: f32 = 112.;
const LIST_MAX_W: f32 = 1344.;

/// The workspace the gallery's fixture draws the start screen in.
const FIXTURE_W: f32 = 1180.;
const FIXTURE_H: f32 = 720.;

/// What a project cover says it is, above the name.
const PROJECT_KIND: &str = "VIDEO";

/// How the start screen lays out the recent projects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum HomeLayout {
    #[default]
    Grid,
    List,
}

/// The start screen's own state: the search field and the layout choice.
pub(super) struct Home {
    search: Entity<TextInputState>,
    layout: HomeLayout,
    _search_subscription: gpui::Subscription,
}

impl Home {
    pub(super) fn new(window: &mut Window, cx: &mut Context<StudioShell>) -> Self {
        let search = cx.new(|cx| TextInputState::single_line(window, cx));
        let subscription = cx.subscribe(
            &search,
            |_, input, event: &TextInputEvent, cx| match event {
                TextInputEvent::Changed => cx.notify(),
                // Escape clears a query before it does anything else.
                TextInputEvent::Cancelled if !input.read(cx).is_empty() => {
                    input.update(cx, |input, cx| input.set_text("", cx));
                }
                _ => {}
            },
        );
        Self {
            search,
            layout: HomeLayout::default(),
            _search_subscription: subscription,
        }
    }
}

impl StudioShell {
    /// The start screen: the dock, the top bar and the recent projects on a dotted surface.
    pub(super) fn render_empty_state(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let recents = &self.presentation.recents;
        let query = self.home.search.read(cx).text().to_owned();
        let shown: Vec<&Recent> = recents
            .iter()
            .filter(|recent| matches_query(&query, &recent.name, &shown_path(&recent.path)))
            .collect();
        let placeholder = search_placeholder(recents.len());
        self.home.search.update(cx, |input, _| {
            input.set_placeholder(placeholder);
        });

        let body = if recents.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new(&theme, "No projects yet")
                        .body(
                            "Create a video with + in the dock, open a project folder or \
                             import a Rust video crate.",
                        )
                        .centered(),
                )
                .into_any_element()
        } else if shown.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new(&theme, "No matching projects")
                        .body(format!("Nothing matches \u{201c}{}\u{201d}.", query.trim()))
                        .centered(),
                )
                .into_any_element()
        } else {
            match self.home.layout {
                HomeLayout::Grid => self.render_shelf(&shown, cx),
                HomeLayout::List => self.render_table(&shown, cx),
            }
        };

        div()
            .id("empty-state")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.colors.window)
            .child(dot_grid(&theme))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(self.render_top_bar(recents.is_empty(), cx))
                    .child(
                        div()
                            .id("recents")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .pl(px(DOCK_CLEARANCE + METRICS.space_4))
                            .pr(px(METRICS.space_8))
                            .pt(px(METRICS.space_4))
                            .pb(px(METRICS.space_8))
                            .child(body),
                    ),
            )
            .child(self.render_dock(cx))
            .into_any_element()
    }

    /// The brand, the search field and the grid or list switch.
    fn render_top_bar(&self, empty: bool, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let layout = self.home.layout;
        let options = vec![
            SegmentOption::icon(Icon::LayoutGrid.path(), layout == HomeLayout::Grid)
                .named("Grid")
                .id("home-layout-grid"),
            SegmentOption::icon(Icon::List.path(), layout == HomeLayout::List)
                .named("List")
                .id("home-layout-list"),
        ];
        let entity = cx.entity();
        let switch =
            SegmentedControl::new(&theme, "home-layout", options).on_select(move |index, _, cx| {
                let layout = if *index == 0 {
                    HomeLayout::Grid
                } else {
                    HomeLayout::List
                };
                entity.update(cx, |shell, cx| {
                    if shell.home.layout != layout {
                        shell.home.layout = layout;
                        cx.notify();
                    }
                });
            });
        div()
            .flex_none()
            .h(px(TOP_BAR_H))
            .flex()
            .items_center()
            .gap(px(METRICS.space_4))
            .px(px(METRICS.space_8))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .child(
                        gpui::img("branding/studio-icon.png")
                            .size(px(METRICS.layout.header_logo))
                            .flex_none(),
                    )
                    .child(
                        text_style(div(), TextStyle::Heading)
                            .text_color(theme.colors.text_1)
                            .truncate()
                            .child("fframes Studio"),
                    ),
            )
            // The search takes the free middle, at most Oleafly's 42rem, centred in it.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .when(!empty, |middle| {
                        middle.child(
                            div().w_full().max_w(px(SEARCH_MAX_W)).flex().child(
                                TextInput::new(&theme, "home-search", &self.home.search)
                                    .search()
                                    .height(px(SEARCH_H))
                                    .radius(px(DOCK_RADIUS)),
                            ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .when(!empty, |row| row.child(switch)),
            )
    }

    /// The floating dock at the left edge: Create, Open, Import, the theme and Settings.
    fn render_dock(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let enabled = !self.busy;
        let current = studio_theme::current_preference(cx);
        let (theme_glyph, theme_label) = theme_toggle(current);
        let size = IconButtonSize::Lg;
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .pl(px(METRICS.space_4))
            .flex()
            .items_center()
            .child(
                div()
                    .id("home-dock")
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .p(px(DOCK_PAD))
                    .rounded_2xl()
                    .border(px(METRICS.size.divider))
                    .border_color(theme.colors.line_strong)
                    .bg(theme.colors.window)
                    .child(
                        self.action_icon_button(
                            "create",
                            "Create…",
                            Icon::Plus,
                            size,
                            enabled,
                            cx,
                            |s, cx| s.open_create(cx),
                        )
                        .round()
                        .emphasis(Emphasis::Primary),
                    )
                    .child(
                        self.action_icon_button(
                            "open",
                            "Open folder…",
                            Icon::FolderOpen,
                            size,
                            enabled,
                            cx,
                            |s, cx| s.pick(Picker::Open, cx),
                        )
                        .round(),
                    )
                    .child(
                        self.action_icon_button(
                            "import",
                            "Import Rust…",
                            Icon::FolderInput,
                            size,
                            enabled,
                            cx,
                            |s, cx| s.pick(Picker::Import, cx),
                        )
                        .round(),
                    )
                    .child(
                        self.action_icon_button(
                            "home-theme",
                            theme_label,
                            theme_glyph,
                            size,
                            true,
                            cx,
                            move |s, cx| s.choose_theme(next_theme(current), cx),
                        )
                        .round(),
                    )
                    .child(
                        self.action_icon_button(
                            "home-settings",
                            "Settings",
                            Icon::Settings,
                            size,
                            true,
                            cx,
                            |s, cx| s.open_settings(cx),
                        )
                        .round(),
                    ),
            )
    }

    /// The recent projects as book covers, in rows that wrap.
    fn render_shelf(&self, shown: &[&Recent], cx: &mut Context<Self>) -> gpui::AnyElement {
        let books: Vec<_> = shown.iter().map(|recent| self.book(recent, cx)).collect();
        div()
            .w_full()
            .max_w(px(SHELF_MAX_W))
            .mx_auto()
            .flex()
            .flex_wrap()
            .gap_x(px(SHELF_GAP_X))
            .gap_y(px(SHELF_GAP_Y))
            .children(books)
            .into_any_element()
    }

    /// One project as a book: the cover opens it, the caption below holds its path and
    /// the row's actions.
    fn book(&self, recent: &Recent, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let enabled = !self.busy;
        let id = String::from(recent.id.clone());
        let path = shown_path(&recent.path);
        let cover = if recent.missing {
            studio_theme::missing_cover(theme.appearance)
        } else {
            studio_theme::book_cover(cover_index(&recent.name, &recent.path))
        };
        let group = gpui::SharedString::from(format!("recent-book-{id}"));
        let disabled = recent.missing || !enabled;

        let face = div()
            .absolute()
            .top_0()
            .left_0()
            .w(px(BOOK_W))
            .h(px(BOOK_H))
            .flex()
            .overflow_hidden()
            .rounded_md()
            .border(px(METRICS.size.divider))
            .border_color(cover.outline)
            .bg(cover.face)
            .child(
                div()
                    .flex_none()
                    .w(px(BOOK_SPINE_W))
                    .h_full()
                    .bg(cover.spine),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap(px(METRICS.space_1))
                    .p(px(METRICS.space_3))
                    .child(
                        text_style(div(), TextStyle::Caption)
                            .text_color(cover.ink_muted)
                            .child(PROJECT_KIND),
                    )
                    .child(
                        text_style(div(), TextStyle::BodyStrong)
                            .text_color(cover.ink)
                            .line_clamp(3)
                            .child(recent.name.clone()),
                    ),
            )
            .when(recent.missing, |face| {
                face.child(
                    div()
                        .absolute()
                        .top(px(METRICS.space_2))
                        .right(px(METRICS.space_2))
                        .child(Badge::new("Missing").tone(Tone::Warning)),
                )
            });

        let open = {
            let entity = cx.entity();
            let target = recent.path.clone();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |shell, cx| {
                    shell.dispatch(Command::Open(target.clone()), cx)
                });
            }) as crate::kit::Handler
        };
        let book = focusable(div().id(format!("recent-{id}")), &theme, disabled)
            .relative()
            .w(px(BOOK_W))
            .h(px(BOOK_H + BOOK_EDGE))
            .rounded_md()
            .when(!disabled, |book| book.cursor_pointer())
            // The back cover, peeking below the front one: the book's thickness.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(BOOK_EDGE))
                    .w(px(BOOK_W))
                    .h(px(BOOK_H))
                    .rounded_md()
                    .bg(cover.spine),
            )
            .child(face);
        let book = on_activate(book, disabled, Some(open));
        let book = with_tooltip(book, &theme, Some(path.clone().into()));

        let actions = self.recent_actions(recent, cx);
        div()
            .id(group.clone())
            .group(group.clone())
            .flex_none()
            .w(px(BOOK_W))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(book)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_1))
                    .min_h(px(METRICS.size.icon_button_sm))
                    .child(
                        text_style(div(), TextStyle::Small)
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.colors.text_2)
                            .text_ellipsis_start()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .child(path),
                    )
                    .child(reveal_unless(actions, recent.missing, group)),
            )
            .into_any_element()
    }

    /// The recent projects as table rows under NAME, PATH and STATUS headings.
    fn render_table(&self, shown: &[&Recent], cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let heading = |text: &'static str| {
            text_style(div(), TextStyle::Overline)
                .text_color(theme.colors.text_2)
                .child(text)
        };
        let header = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_4))
            .h(px(METRICS.size.list_row_h + METRICS.space_2))
            .px(px(METRICS.space_4))
            .border_b(px(METRICS.size.divider))
            .border_color(theme.colors.line)
            .child(div().flex_1().min_w_0().child(heading("NAME")))
            .child(div().flex_1().min_w_0().child(heading("PATH")))
            .child(
                div()
                    .flex_none()
                    .w(px(LIST_STATUS_W))
                    .child(heading("STATUS")),
            )
            .child(div().flex_none().w(px(LIST_ACTIONS_W)));
        let rows: Vec<_> = shown
            .iter()
            .map(|recent| self.table_row(recent, cx))
            .collect();
        div()
            .w_full()
            .max_w(px(LIST_MAX_W))
            .mx_auto()
            .flex()
            .flex_col()
            .child(header)
            .children(rows)
            .into_any_element()
    }

    /// One project as a table row: the row opens it, the actions show on hover.
    fn table_row(&self, recent: &Recent, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let enabled = !self.busy;
        let id = String::from(recent.id.clone());
        let path = shown_path(&recent.path);
        let cover = if recent.missing {
            studio_theme::missing_cover(theme.appearance)
        } else {
            studio_theme::book_cover(cover_index(&recent.name, &recent.path))
        };
        let group = gpui::SharedString::from(format!("recent-row-{id}"));
        let disabled = recent.missing || !enabled;
        let open = {
            let entity = cx.entity();
            let target = recent.path.clone();
            std::rc::Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                entity.update(cx, |shell, cx| {
                    shell.dispatch(Command::Open(target.clone()), cx)
                });
            }) as crate::kit::Handler
        };
        let name_color = if recent.missing {
            theme.colors.text_2
        } else {
            theme.colors.text_1
        };
        let status = if recent.missing {
            Badge::new("Missing").tone(Tone::Warning).into_any_element()
        } else {
            text_style(div(), TextStyle::Small)
                .text_color(theme.colors.text_2)
                .child("Ready")
                .into_any_element()
        };
        let row = focusable(div().id(format!("recent-{id}")), &theme, disabled)
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(METRICS.space_4))
            .h(px(LIST_ROW_H))
            .px(px(METRICS.space_4))
            .border_b(px(METRICS.size.divider))
            .border_color(theme.colors.line)
            .rounded_md()
            .hover(|row| row.bg(theme.colors.ctl))
            .when(!disabled, |row| row.cursor_pointer())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_3))
                    .child(
                        div()
                            .flex_none()
                            .w(px(LIST_THUMB_W))
                            .h(px(LIST_THUMB_H))
                            .flex()
                            .overflow_hidden()
                            .rounded_sm()
                            .border(px(METRICS.size.divider))
                            .border_color(cover.outline)
                            .bg(cover.face)
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(BOOK_SPINE_W / 2.))
                                    .h_full()
                                    .bg(cover.spine),
                            ),
                    )
                    .child(
                        text_style(div(), TextStyle::BodyMedium)
                            .min_w_0()
                            .text_color(name_color)
                            .truncate()
                            .child(recent.name.clone()),
                    ),
            )
            .child(
                text_style(div(), TextStyle::Small)
                    .flex_1()
                    .min_w_0()
                    .text_color(theme.colors.text_2)
                    .text_ellipsis_start()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(path.clone()),
            )
            .child(div().flex_none().w(px(LIST_STATUS_W)).flex().child(status))
            .child(
                div()
                    .flex_none()
                    .w(px(LIST_ACTIONS_W))
                    .flex()
                    .justify_end()
                    .child(reveal_unless(
                        self.recent_actions(recent, cx),
                        recent.missing,
                        group,
                    )),
            );
        let row = on_activate(row, disabled, Some(open));
        with_tooltip(row, &theme, Some(path.into())).into_any_element()
    }

    /// A recent project's actions: Locate… for a missing one, Remove for every one. A
    /// press on them is theirs, not the row's or the cover's.
    fn recent_actions(&self, recent: &Recent, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let enabled = !self.busy;
        let id = String::from(recent.id.clone());
        let mut actions = div()
            .id(format!("recent-actions-{id}"))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_1))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation());
        for action in recent_row_actions(recent.missing) {
            let entity = cx.entity();
            let target = recent.id.clone();
            actions = actions.child(match action {
                "locate" => Button::new(&theme, format!("locate-{id}"))
                    .label("Locate…")
                    .size(ButtonSize::Sm)
                    .emphasis(Emphasis::Neutral)
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |shell, cx| {
                            shell.pick(Picker::Locate(target.clone()), cx)
                        });
                    })
                    .into_any_element(),
                _ => IconButton::new(&theme, format!("remove-{id}"), Icon::X.path())
                    .size(IconButtonSize::Sm)
                    .label("Remove recent")
                    .tooltip("Remove recent")
                    .disabled(!enabled)
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |shell, cx| {
                            shell.dispatch(Command::RemoveRecent(target.clone()), cx)
                        });
                    })
                    .into_any_element(),
            });
        }
        actions
    }

    /// The compact recent-projects list of the Project tab, shown while a project is open
    /// (the start screen has its own shelf then, so the element ids stay unique).
    pub(super) fn render_recents(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let enabled = !self.busy;
        let rows = self.presentation.recents.clone().into_iter().map(|recent| {
            let id = String::from(recent.id.clone());
            let shown_path = shown_path(&recent.path);
            let row = ListRow::new(&theme, format!("recent-{id}"))
                .icon(Icon::Folder)
                .title(recent.name.clone())
                .meta(shown_path.clone())
                .tooltip(shown_path)
                .action(self.recent_actions(&recent, cx));
            if recent.missing {
                // It cannot open; its actions still locate or remove it.
                row.muted(true)
                    .trailing(Badge::new("Missing").tone(Tone::Warning))
            } else {
                let entity = cx.entity();
                let path = recent.path.clone();
                row.disabled(!enabled).on_click(move |_, cx| {
                    entity.update(cx, |shell, cx| {
                        shell.dispatch(Command::Open(path.clone()), cx)
                    });
                })
            }
        });
        div().id("recents").child(
            List::new()
                .children(rows.collect::<Vec<_>>())
                .empty(EmptyState::new(&theme, "No recent projects")),
        )
    }

    /// The start screen at the gallery's size in state `state` (0 a grid of recent
    /// projects, a missing one among them; 1 the same as a list; 2 filtered by a search;
    /// 3 no recent projects), drawn with `theme`. For the UI gallery's captures; it changes
    /// the shell's state, so the shell is the gallery's own.
    pub fn render_home_fixture(
        &mut self,
        state: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.presentation.project = None;
        self.presentation.recents = if state == 3 {
            Vec::new()
        } else {
            fixture_recents()
        };
        self.home.layout = if state == 1 {
            HomeLayout::List
        } else {
            HomeLayout::Grid
        };
        let query = if state == 2 { "launch" } else { "" };
        self.home.search.update(cx, |input, cx| {
            if input.text() != query {
                input.set_text(query, cx);
            }
        });
        let main = std::mem::replace(&mut self.theme, theme);
        let home = self.render_empty_state(cx);
        self.theme = main;
        // A window's workspace of its own: the gallery's column scrolls, so it has no height
        // to fill.
        div()
            .flex_none()
            .w(px(FIXTURE_W))
            .h(px(FIXTURE_H))
            .flex()
            .border(px(METRICS.size.divider))
            .border_color(theme.colors.line)
            .child(home)
            .into_any_element()
    }
}

/// Recent projects for the gallery: several names and paths, one of them missing.
fn fixture_recents() -> Vec<Recent> {
    [
        (
            "launch-video",
            "Launch video",
            "C:/Videos/launch-video",
            false,
        ),
        (
            "product-tour",
            "Product tour",
            "C:/Videos/product-tour",
            false,
        ),
        (
            "q3-review",
            "Quarterly review deck",
            "D:/Work/q3-review",
            true,
        ),
        ("lyric-video", "Lyric video", "C:/Music/lyric-video", false),
        (
            "launch-teaser",
            "Launch teaser, 15 seconds",
            "C:/Videos/launch-teaser",
            false,
        ),
        ("podcast-intro", "Podcast intro", "C:/Podcast/intro", false),
    ]
    .into_iter()
    .map(|(id, name, path, missing)| Recent {
        id: studio_project::ProjectId::try_from(id.to_owned()).expect("a valid id"),
        name: name.into(),
        path: path.into(),
        missing,
    })
    .collect()
}

/// The path a recent project shows: where it is outside the app's sandbox.
fn shown_path(path: &Path) -> String {
    studio_bootstrap::external_path(path).display().to_string()
}

/// The search field's placeholder for `count` projects.
fn search_placeholder(count: usize) -> String {
    let noun = if count == 1 { "project" } else { "projects" };
    format!("Search {count} {noun} by name or path")
}

/// Whether a project with `name` at `path` matches the search `query`: every word of the
/// query appears in the name or the path, ignoring case. An empty query matches all.
fn matches_query(query: &str, name: &str, path: &str) -> bool {
    let name = name.to_lowercase();
    let path = path.to_lowercase();
    query
        .split_whitespace()
        .map(str::to_lowercase)
        .all(|word| name.contains(&word) || path.contains(&word))
}

/// The cover colour of a project: a stable hash (FNV-1a, the same on every run and
/// platform) of its name and path into the cover palette.
fn cover_index(name: &str, path: &Path) -> usize {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let path = path.to_string_lossy();
    let bytes = name.bytes().chain([0]).chain(path.bytes());
    let hash = bytes.fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    });
    // The high bits are the better mixed ones.
    ((hash >> 32) % studio_theme::COVER_COUNT as u64) as usize
}

/// The dock's theme button for the current preference: its glyph and its name.
fn theme_toggle(current: AppTheme) -> (Icon, &'static str) {
    match current {
        AppTheme::System => (Icon::Monitor, "Theme: System"),
        AppTheme::Light => (Icon::Sun, "Theme: Light"),
        AppTheme::Dark => (Icon::Moon, "Theme: Dark"),
    }
}

/// The preference the theme button moves to: System, Light, Dark, then System again,
/// the order of the Settings control.
fn next_theme(current: AppTheme) -> AppTheme {
    match current {
        AppTheme::System => AppTheme::Light,
        AppTheme::Light => AppTheme::Dark,
        AppTheme::Dark => AppTheme::System,
    }
}

/// The actions of a recent-project row, as the id prefix of each button: a missing
/// project can be located, every project can be removed.
fn recent_row_actions(missing: bool) -> Vec<&'static str> {
    if missing {
        vec!["locate", "remove"]
    } else {
        vec!["remove"]
    }
}

/// A recent project's actions, hidden until hover or keyboard focus within `group`, or
/// always shown when `always` (a missing project, whose actions are its only use).
fn reveal_unless(
    actions: gpui::Stateful<gpui::Div>,
    always: bool,
    group: gpui::SharedString,
) -> gpui::Stateful<gpui::Div> {
    if always {
        actions
    } else {
        reveal_actions(actions, group)
    }
}

/// The dotted surface behind the start screen: a dot every [`DOT_PITCH`] in `line_strong`
/// (Oleafly's dot pattern; white at 15 % in dark, close to its 17 %).
fn dot_grid(theme: &Theme) -> impl IntoElement {
    let color = theme.colors.line_strong;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let pitch = px(DOT_PITCH);
            let dot = size(px(DOT_SIZE), px(DOT_SIZE));
            let origin = bounds.origin + point(pitch / 2., pitch / 2.);
            let columns = (f32::from(bounds.size.width) / DOT_PITCH).ceil() as usize;
            let rows = (f32::from(bounds.size.height) / DOT_PITCH).ceil() as usize;
            for row in 0..rows {
                for column in 0..columns {
                    let at = origin + point(pitch * column as f32, pitch * row as f32);
                    window.paint_quad(
                        fill(Bounds::new(at, dot), color).corner_radii(px(DOT_SIZE / 2.)),
                    );
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// FNV-1a of "Launch video", a zero byte and "C:/Videos/launch-video", worked out apart
    /// from this code: its high 32 bits modulo the 10 covers.
    const PINNED_LAUNCH_VIDEO: usize = 1;

    #[test]
    fn recent_rows_offer_locate_only_when_missing() {
        assert_eq!(recent_row_actions(true), ["locate", "remove"]);
        assert_eq!(recent_row_actions(false), ["remove"]);
    }

    #[test]
    fn a_cover_colour_is_stable_for_a_project() {
        let path = PathBuf::from("C:/Videos/launch-video");
        let first = cover_index("Launch video", &path);
        for _ in 0..3 {
            assert_eq!(cover_index("Launch video", &path), first);
        }
        // Pinned: FNV-1a gives the same colour on every run, build and platform, where the
        // std hasher's keys may change.
        assert_eq!(first, PINNED_LAUNCH_VIDEO);
        // The path tells two projects of one name apart, and the name two of one path.
        let names: std::collections::HashSet<_> = (0..8)
            .map(|i| cover_index("Untitled", Path::new(&format!("C:/Videos/untitled-{i}"))))
            .collect();
        assert!(names.len() > 1, "{names:?}");
    }

    #[test]
    fn cover_colours_spread_over_the_palette() {
        let count = studio_theme::COVER_COUNT;
        let projects = 1000;
        let mut buckets = vec![0usize; count];
        for i in 0..projects {
            let name = format!("Project {i}");
            buckets[cover_index(&name, Path::new(&format!("C:/Videos/project-{i}")))] += 1;
        }
        let expected = projects / count;
        for (index, used) in buckets.iter().enumerate() {
            assert!(
                *used > expected / 2 && *used < expected * 2,
                "cover {index} got {used} of {projects}: {buckets:?}"
            );
        }
    }

    #[test]
    fn search_matches_every_word_in_the_name_or_the_path() {
        let (name, path) = ("Launch Video", "C:/Videos/launch-video");
        assert!(matches_query("", name, path));
        assert!(matches_query("   ", name, path));
        assert!(matches_query("launch", name, path));
        assert!(matches_query("VIDEO", name, path));
        assert!(matches_query("videos/", name, path), "the path counts");
        assert!(matches_query("  launch   video ", name, path));
        assert!(
            !matches_query("launch tour", name, path),
            "every word must match"
        );
        assert!(!matches_query("teaser", name, path));
    }

    #[test]
    fn search_placeholder_counts_the_projects() {
        assert_eq!(search_placeholder(1), "Search 1 project by name or path");
        assert_eq!(search_placeholder(6), "Search 6 projects by name or path");
    }

    #[test]
    fn the_theme_button_cycles_like_the_settings_control() {
        let mut theme = AppTheme::System;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(theme_toggle(theme).0);
            theme = next_theme(theme);
        }
        assert_eq!(theme, AppTheme::System);
        assert_eq!(seen, [Icon::Monitor, Icon::Sun, Icon::Moon]);
    }
}
