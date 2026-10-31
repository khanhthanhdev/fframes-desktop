//! The agent panel's chat components (`plans/261027-2015-agent-panel-components/plan.md`).
//!
//! Each is a `RenderOnce` builder over plain values, like the kit: the panel maps its
//! workflow snapshot onto them, and the UI gallery renders them from fixtures. They read the
//! theme's roles, `METRICS` and `TextStyle` only.
//!
//! - [`UserMessage`] and [`AssistantMessage`]: the two voices of the transcript, an accent
//!   bubble and a framed answer, each with a footer for the time and Copy that is always
//!   reserved.
//! - [`ActivityHeader`] and [`ActivityRow`]: a run of tool calls, reasoning and permission
//!   notes as one quiet summary line, opened into rows.
//! - [`ChangesCard`], [`ActionCard`]: files a task changed; everything that waits for the user.
//! - [`tray_frame`], [`TrayHeader`], [`TrayRow`]: the todo list and the queue above the composer.
//! - [`StatusLine`], [`PanelHeader`], [`WorkingTrailer`], [`JumpPill`], [`panel_empty`].
//! - [`outcome_style`]: the one outcome vocabulary of the transcript, status line and History.
//! - [`metrics`]: the panel's own denser sizes.

mod action_card;
mod activity;
mod changes_card;
mod chrome;
mod message;
pub mod metrics;
mod outcome;
mod tray;

pub use action_card::ActionCard;
pub use activity::{ActivityCounts, ActivityHeader, ActivityRow, ActivityState, summary_parts};
pub use changes_card::{ChangeMark, ChangedFile, ChangesCard};
pub use chrome::{
    JumpPill, PanelHeader, StatusLine, WorkingTrailer, elapsed_label, panel_empty, panel_home,
};
pub use message::{AssistantMessage, USER_MESSAGE_MAX_LINES, UserMessage, estimated_lines};
pub use outcome::outcome_style;
pub use tray::{TRAY_MAX_H, TrayHeader, TrayRow, TrayRowState, tray_frame};

use gpui::{Pixels, px};
use studio_theme::METRICS;

/// One row of a list in the panel: tray rows, changed files, outcome lines. Activity rows use
/// [`metrics::activity::ROW_H`].
pub const ROW_H: f32 = METRICS.size.control_sm_h;
/// A status mark at the end of a row: the failed cross, the done check.
pub const STATUS_MARK: f32 = 12.;
/// Corner of a row inside the panel (activity rows, hover fills).
pub const ITEM_RADIUS: Pixels = px(METRICS.radius.item);
/// Corner of a bubble or card inside the panel.
pub const CARD_RADIUS: Pixels = px(METRICS.radius.card);
