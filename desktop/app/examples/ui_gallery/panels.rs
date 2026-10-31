//! The props, presets and shell capture pages (`--page props|presets|shell`): the Props tab's
//! `PropsPanel` and the Style tab's `PresetPanel` fed fixture snapshots, and the window's
//! chrome (header, notices, sidebar tab rail, transport, status bar) followed by the Project,
//! Assets and Style tabs at the sidebar's narrowest and widest, drawn by `StudioShell`.

use std::collections::BTreeMap;

use fframes_studio_protocol::{
    EditorObjectIdentity, PropField, PropKind, PropsDocument, PropsInfo, PropsSchema,
};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement, Styled, Window, div,
    px,
};
use serde_json::json;
use studio_engine::PresetAction;
use studio_engine::PresetMutation;
use studio_engine::presets::{
    AppliedView, CssView, EntryView, Outcome, PresetReport, PresetView, Refusal,
};
use studio_engine::props::{PropsEditing, PropsSnapshot};
use studio_theme::{METRICS, Theme};
use studio_ui::kit::Card;
use studio_ui::preset_panel::PresetPanel;
use studio_ui::props_panel::PropsPanel;

use crate::Gallery;
use crate::capture::Page;

/// The width the presets panel is drawn in.
const PANEL_W: f32 = 340.;

/// The height the props panels scroll inside, as the sidebar gives them.
const PROPS_H: f32 = 600.;

/// The panels, made on first use, and the page, state and theme they were last set up for.
#[derive(Default)]
pub struct PanelFixtures {
    props: Option<[Entity<PropsPanel>; 2]>,
    presets: Option<Entity<PresetPanel>>,
    shown: Option<(Page, &'static str, Theme)>,
}

impl Gallery {
    /// The props, presets and shell pages in their fixture state; `None` on the other pages.
    pub(crate) fn render_panels(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let page = self.page.filter(|page| {
            matches!(page, Page::Props | Page::Presets | Page::Shell | Page::Home)
        })?;
        let state = self.phase7.state;
        let index = page.states().iter().position(|known| *known == state)?;
        if page == Page::Home {
            let shell = self.fixture_shell(window, cx);
            let theme = *theme;
            return Some(shell.update(cx, |shell, cx| shell.render_home_fixture(index, theme, cx)));
        }
        if page == Page::Shell {
            let shell = self.fixture_shell(window, cx);
            let theme = *theme;
            // The chrome, then the Project, Assets and Style tabs (which have no page of their
            // own) in the same project state.
            let (chrome, drawer) = shell.update(cx, |shell, cx| {
                let chrome = shell.render_shell_fixture(index, theme, cx);
                (chrome, shell.render_drawer_fixture(theme, cx))
            });
            return Some(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(chrome)
                    .child(drawer)
                    .into_any_element(),
            );
        }
        let fixtures = &mut self.panels;
        let changed = fixtures.shown != Some((page, state, *theme));
        fixtures.shown = Some((page, state, *theme));
        if page == Page::Props {
            // The Props tab at the sidebar's narrowest and widest, one panel each.
            let panels = fixtures
                .props
                .get_or_insert_with(|| [(); 2].map(|_| cx.new(|cx| PropsPanel::new(window, cx))))
                .clone();
            let mut row = div().flex().items_start().gap(px(METRICS.space_4));
            for (panel, width) in panels
                .into_iter()
                .zip([METRICS.layout.sidebar_min_w, METRICS.layout.sidebar_max_w])
            {
                if changed {
                    set_up_props(&panel, state, cx);
                }
                row = row.child(
                    Card::panel(theme)
                        .bg(theme.colors.sidebar)
                        .w(px(width))
                        .h(px(PROPS_H))
                        .flex()
                        .flex_col()
                        .p(px(METRICS.space_4))
                        .child(panel),
                );
            }
            return Some(row.into_any_element());
        }
        let panel = fixtures
            .presets
            .get_or_insert_with(|| cx.new(|cx| PresetPanel::new(window, cx)))
            .clone();
        if changed {
            set_up_presets(&panel, state, cx);
        }
        // The presets panel is as tall as its content.
        let card = Card::raised(theme)
            .w(px(PANEL_W))
            .flex()
            .flex_col()
            .flex_none()
            .p(px(METRICS.space_4));
        Some(card.child(panel).into_any_element())
    }
}

fn identity() -> EditorObjectIdentity {
    EditorObjectIdentity {
        scene_instance_key: "intro".into(),
        component_key: "LowerThird".into(),
        object_key: "title".into(),
        repeat_key: "main".into(),
    }
}

fn field(name: &str, kind: PropKind) -> PropField {
    PropField {
        name: name.into(),
        kind,
        min: None,
        max: None,
        step: None,
        unit: None,
    }
}

/// One of every kind of field, two of them edited away from the code's value.
fn props_info() -> PropsInfo {
    let choice = |options: &[&str]| PropKind::Choice {
        options: options.iter().map(|option| option.to_string()).collect(),
    };
    let fields = vec![
        field("title", PropKind::Text),
        field("size", PropKind::Number),
        PropField {
            min: Some(0.),
            max: Some(1.),
            step: Some(0.05),
            ..field("opacity", PropKind::Number)
        },
        PropField {
            min: Some(12.),
            max: Some(96.),
            step: Some(1.),
            unit: Some("px".into()),
            ..field("font_size", PropKind::Integer)
        },
        field("visible", PropKind::Toggle),
        field("accent", PropKind::Color),
        field("align", choice(&["left", "center", "right"])),
        field(
            "font",
            choice(&["Inter", "Geist", "Roboto", "Lora", "Fira Code", "Playfair"]),
        ),
    ];
    let values = BTreeMap::from([
        ("title".to_owned(), json!("Launch day")),
        ("size".to_owned(), json!(1.5)),
        ("opacity".to_owned(), json!(0.8)),
        ("font_size".to_owned(), json!(48)),
        ("visible".to_owned(), json!(true)),
        ("accent".to_owned(), json!("#ff5533")),
        ("align".to_owned(), json!("center")),
        ("font".to_owned(), json!("Geist")),
    ]);
    let mut code_values = values.clone();
    code_values.insert("title".into(), json!("Hello"));
    code_values.insert("accent".into(), json!("#3366ff"));
    PropsInfo {
        schema: PropsSchema {
            component: "LowerThird".into(),
            fields,
        },
        code_values,
        values,
    }
}

fn set_up_props(panel: &Entity<PropsPanel>, state: &str, cx: &mut Context<Gallery>) {
    let snapshot = (state != "no-project").then(|| PropsSnapshot {
        generation: 1,
        props_json: None,
        doc: Some(PropsDocument::default()),
        can_undo: state == "fields",
        can_redo: false,
        editing: if state == "disabled" {
            PropsEditing::Disabled("an agent task is running".into())
        } else {
            PropsEditing::Enabled
        },
        notice: (state == "disabled").then(|| "props.json was reloaded from disk".into()),
    });
    let selected = !matches!(state, "no-project" | "no-selection");
    let info: Result<Option<PropsInfo>, String> = match state {
        "error" => Err("the preview worker did not answer: timed out after 5 s".into()),
        "not-editable" => Ok(None),
        _ => Ok(Some(props_info())),
    };
    panel.update(cx, |panel, cx| {
        panel.set_snapshot(snapshot, cx);
        panel.set_selection(selected.then(identity), cx);
        if selected {
            panel.set_info(identity(), info, cx);
        }
    });
}

fn entry(key: &str, name: &str, description: &str, bundled: bool, applied: bool) -> EntryView {
    EntryView {
        key: key.into(),
        id: name.to_lowercase().replace(' ', "-"),
        name: name.into(),
        version: "1.2.0".into(),
        description: description.into(),
        bundled,
        applied,
        same_id_applied: applied,
    }
}

fn preset_view(state: &str) -> PresetView {
    let applied_entry = state != "unapplied";
    let entries = if state == "empty" {
        Vec::new()
    } else {
        vec![
            entry(
                &"a".repeat(64),
                "Studio",
                "Clean sans-serif titles on a dark ground.",
                true,
                applied_entry,
            ),
            entry(
                &"b".repeat(64),
                "Paper",
                "Warm light background with serif headings.",
                true,
                false,
            ),
            entry(
                &"c".repeat(64),
                "Neon",
                "High-contrast colours for short clips.",
                true,
                false,
            ),
            entry(
                &"d".repeat(64),
                "Brand kit",
                "Imported from brand/preset.",
                false,
                false,
            ),
        ]
    };
    let applied = (applied_entry && state != "empty").then(|| AppliedView {
        id: "studio".into(),
        name: "Studio".into(),
        version: "1.2.0".into(),
        hash: "a".repeat(64),
        overridden: vec![
            ("color.accent".into(), "light".into(), "\"#ff5533\"".into()),
            ("space.title".into(), "base".into(), "120".into()),
        ],
        diagnostics: vec!["color.accnet: no such token in this preset".into()],
    });
    PresetView {
        entries,
        skipped: if state == "list" {
            vec!["old-theme: unsupported preset version 0".into()]
        } else {
            Vec::new()
        },
        applied,
        mutation: if state == "disabled" {
            PresetMutation::Disabled("an agent task is running".into())
        } else {
            PresetMutation::Enabled
        },
    }
}

fn preset_report(state: &str) -> Option<PresetReport> {
    match state {
        "committed" => Some(PresetReport {
            outcome: Outcome::Committed {
                action: PresetAction::Apply,
                source: "7f3a9c1e2b04".into(),
                notes: vec!["2 overrides kept".into()],
            },
            css: Some(CssView {
                file: "brand.css".into(),
                accepted: 6,
                unsupported: 2,
                rejected: 1,
                lines: vec![
                    "--accent: #ff5533; accepted".into(),
                    "--radius: 12px; accepted".into(),
                    "backdrop-filter: blur(4px); unsupported".into(),
                    "--shadow: nonsense; rejected: not a shadow".into(),
                ],
                truncated: true,
            }),
            catalog_changed: false,
        }),
        "refused" | "disabled" => Some(PresetReport {
            outcome: Outcome::Refused(Refusal::SourceChanged(
                "the project changed while the preset was applied".into(),
            )),
            css: None,
            catalog_changed: false,
        }),
        _ => None,
    }
}

fn set_up_presets(panel: &Entity<PresetPanel>, state: &str, cx: &mut Context<Gallery>) {
    let view = (state != "no-project").then(|| preset_view(state));
    let report = preset_report(state);
    panel.update(cx, |panel, cx| {
        panel.set_view(None, cx);
        panel.set_view(view, cx);
        if let Some(report) = report {
            panel.report(report, cx);
        }
    });
}
