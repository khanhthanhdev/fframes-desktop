//! The disk space the managed SDK and the build caches use: removing the caches of SDKs that
//! were replaced by itself (once per launch and after each SDK install, while no project is
//! open), and Settings → SDK's Storage section with **Clean up** and **Remove previous SDK**.
use gpui::{Context, IntoElement};
use studio_engine::engine::backend::Command;
use studio_engine::storage::{StorageUsage, format_size};

use super::StudioShell;
use crate::kit::{PanelSection, SettingRow};

/// What the Storage section shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum StorageState {
    #[default]
    NotMeasured,
    Measuring,
    /// Removing the unused caches; the bytes freed show once it is done.
    Cleaning,
    Measured {
        usage: StorageUsage,
        /// Bytes the last clean-up freed this launch.
        freed: Option<u64>,
    },
}

impl StudioShell {
    /// Removes the unused build caches in the background, once per launch and again after
    /// an SDK install. Only while no project is open and no command or shipped SDK install
    /// is pending, so the cache a starting build needs is never the one being removed.
    pub(super) fn clean_storage_automatically(&mut self, cx: &mut Context<Self>) {
        let sdk_install_pending = !self.sdk_auto_update_tried
            && !self.sdk_auto_update_disabled
            && self.presentation.sdk_update.is_some();
        if self.storage_cleaned
            || !self.setup_checked
            || self.busy
            || self.presentation.project.is_some()
            || sdk_install_pending
            || matches!(self.storage, StorageState::Cleaning)
        {
            return;
        }
        self.storage_cleaned = true;
        self.clean_storage(cx);
    }

    /// Measures what the SDKs and the build caches use, off the UI thread.
    pub(super) fn measure_storage(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.storage,
            StorageState::Measuring | StorageState::Cleaning
        ) {
            return;
        }
        self.run_storage(false, cx);
    }

    /// Removes the unused build caches, then measures again.
    fn clean_storage(&mut self, cx: &mut Context<Self>) {
        self.run_storage(true, cx);
    }

    fn run_storage(&mut self, clean: bool, cx: &mut Context<Self>) {
        let (Some(sdk_home), Some(paths)) = (
            self.presentation.sdk_home.clone(),
            self.presentation.paths.clone(),
        ) else {
            return;
        };
        // The open project's SDK may be one picked outside the installed ones.
        let keep = self
            .presentation
            .agent_sdk
            .as_ref()
            .map(|sdk| sdk.compatibility.clone());
        self.storage = if clean {
            StorageState::Cleaning
        } else {
            StorageState::Measuring
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let builds = paths.builds();
            let (freed, usage) = cx
                .background_executor()
                .spawn(async move {
                    let freed = clean
                        .then(|| studio_engine::storage::clean(&sdk_home, &builds, keep.as_ref()));
                    let usage = studio_engine::storage::measure(&sdk_home, &builds, keep.as_ref());
                    (freed, usage)
                })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.storage = StorageState::Measured { usage, freed };
                cx.notify();
            });
        })
        .detach();
    }

    /// Settings → SDK's Storage section.
    pub(super) fn render_storage(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let working = matches!(
            self.storage,
            StorageState::Measuring | StorageState::Cleaning
        );
        let (sdk, previous, cache, unused) = match &self.storage {
            StorageState::Measured { usage, freed } => (
                format!("{} for the active SDK.", format_size(usage.sdk)),
                usage.rollback_sdk.map(|size| {
                    format!(
                        "{} for the SDK the last install replaced. Projects pinned to it keep \
                         building until you upgrade them; remove it once they are upgraded.",
                        format_size(size)
                    )
                }),
                format!(
                    "{} of compiled frameworks and projects the installed SDKs still use.",
                    format_size(usage.build_cache)
                ),
                match freed {
                    Some(freed) if *freed > 0 && usage.unused == 0 => {
                        format!("Freed {}. Nothing else to remove.", format_size(*freed))
                    }
                    _ if usage.unused == 0 => "Nothing to remove.".to_owned(),
                    _ => format!(
                        "{} of build caches of replaced SDKs and of builds Studio left \
                         unfinished. Studio removes them by itself at launch and after an SDK \
                         install.",
                        format_size(usage.unused)
                    ),
                },
            ),
            StorageState::Cleaning => {
                let cleaning = "Removing unused build caches…".to_owned();
                (cleaning.clone(), None, cleaning.clone(), cleaning)
            }
            StorageState::NotMeasured | StorageState::Measuring => {
                let measuring = "Measuring…".to_owned();
                (measuring.clone(), None, measuring.clone(), measuring)
            }
        };
        let has_unused = matches!(
            &self.storage,
            StorageState::Measured { usage, .. } if usage.unused > 0
        );
        let mut rows = vec![
            SettingRow::new(&theme, "Managed SDK")
                .description(sdk)
                .into_any_element(),
        ];
        if let Some(previous) = previous {
            rows.push(
                SettingRow::new(&theme, "Previous SDK")
                    .description(previous)
                    .control(self.action_button(
                        "remove-previous-sdk",
                        "Remove",
                        !working && !self.busy,
                        false,
                        None,
                        cx,
                        |shell, cx| shell.dispatch(Command::RemoveRollbackSdk, cx),
                    ))
                    .into_any_element(),
            );
        }
        rows.push(
            SettingRow::new(&theme, "Build cache")
                .description(cache)
                .into_any_element(),
        );
        rows.push(
            SettingRow::new(&theme, "Unused files")
                .description(unused)
                .control(self.action_button(
                    "clean-storage",
                    "Clean up",
                    !working && has_unused,
                    false,
                    None,
                    cx,
                    |shell, cx| shell.clean_storage(cx),
                ))
                .into_any_element(),
        );
        PanelSection::new(&theme, "Storage")
            .description("The disk space the SDK and its build caches use.")
            .children(rows)
            .into_any_element()
    }
}
