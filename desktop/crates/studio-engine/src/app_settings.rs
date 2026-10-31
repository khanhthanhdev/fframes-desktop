//! Preferences of the studio app itself, kept in `app-settings.json` under the app data
//! directory. A missing or unreadable file means the defaults; settings never stop the
//! app from starting.

use std::io::Write;
use std::path::PathBuf;

use fframes_studio_protocol::export::ExportSettings;
use serde::{Deserialize, Serialize};
use studio_proto::{AppTheme, MotionPreference, PermissionPolicy};

use crate::agent_settings::AgentToolSettings;
use crate::app_paths::AppPaths;

pub const APP_SETTINGS_FILE: &str = "app-settings.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// Light, dark, or following the operating system.
    pub theme: AppTheme,
    /// Full, reduced, or following the operating system. Files written before this
    /// existed, and values this app no longer knows, read as `System`.
    #[serde(deserialize_with = "lenient")]
    pub motion: MotionPreference,
    /// The export panel's last choice, for every project. A value that no longer reads
    /// (a format this app dropped) is the default instead of discarding the other settings.
    #[serde(deserialize_with = "lenient")]
    pub export: ExportSettings,
    /// Whether the first-run setup was finished or skipped. `None`: never recorded (a first
    /// launch, or settings written before onboarding existed); see [`needs_onboarding`].
    #[serde(deserialize_with = "lenient")]
    pub onboarding_completed: Option<bool>,
    /// How agent permission requests are answered.
    #[serde(deserialize_with = "lenient")]
    pub permission_policy: PermissionPolicy,
    /// Whether the check for a newer Studio at launch is turned off (it is on by default).
    #[serde(deserialize_with = "lenient")]
    pub update_check_disabled: bool,
    /// The user's own MCP servers and the project tools they turned off.
    #[serde(deserialize_with = "lenient")]
    pub agent: AgentToolSettings,
}

/// A value that no longer reads (for example one this app dropped) is the default, instead
/// of discarding the other settings.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Whether the first-run setup should open. Once recorded, the record decides. Before that
/// (a first launch, or an install from before onboarding existed) it opens only when the
/// managed SDK or the agent is still missing, so a set-up install is never asked again.
pub fn needs_onboarding(settings: &AppSettings, sdk_ready: bool, agent_selected: bool) -> bool {
    match settings.onboarding_completed {
        Some(completed) => !completed,
        None => !(sdk_ready && agent_selected),
    }
}

pub fn app_settings_path(paths: &AppPaths) -> PathBuf {
    paths.data.join(APP_SETTINGS_FILE)
}

impl AppSettings {
    /// The saved settings, or the defaults when none were saved or the file cannot be read.
    pub fn load(paths: &AppPaths) -> Self {
        std::fs::read_to_string(app_settings_path(paths))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Replaces the saved settings through a temporary file renamed over the old one.
    pub fn save(&self, paths: &AppPaths) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Cannot encode the app settings: {e}"))?;
        let mut file = tempfile::NamedTempFile::new_in(&paths.data)
            .map_err(|e| format!("Cannot write the app settings: {e}"))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.as_file().sync_all())
            .map_err(|e| format!("Cannot write the app settings: {e}"))?;
        file.persist(app_settings_path(paths))
            .map_err(|e| format!("Cannot save the app settings: {}", e.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_survives_a_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        assert_eq!(AppSettings::load(&paths), AppSettings::default());
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::System);

        let dark = AppSettings {
            theme: AppTheme::Dark,
            ..AppSettings::default()
        };
        dark.save(&paths).unwrap();
        assert_eq!(AppSettings::load(&paths), dark);
        AppSettings {
            theme: AppTheme::Light,
            ..AppSettings::default()
        }
        .save(&paths)
        .unwrap();
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::Light);
        let leftovers = std::fs::read_dir(&paths.data).unwrap().count();
        assert_eq!(leftovers, 1, "only the settings file remains");
    }

    #[test]
    fn an_unreadable_or_partial_file_means_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        std::fs::write(app_settings_path(&paths), "{ not json").unwrap();
        assert_eq!(AppSettings::load(&paths), AppSettings::default());
        std::fs::write(app_settings_path(&paths), "{}").unwrap();
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::System);
        std::fs::write(app_settings_path(&paths), r#"{"theme":"dark","later":1}"#).unwrap();
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::Dark);
    }

    #[test]
    fn the_export_choice_survives_and_an_invalid_one_keeps_the_theme() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        let settings = AppSettings {
            theme: AppTheme::Dark,
            export: ExportSettings {
                scale: 0.5,
                ..ExportSettings::default()
            },
            ..AppSettings::default()
        };
        settings.save(&paths).unwrap();
        assert_eq!(AppSettings::load(&paths), settings);
        std::fs::write(
            app_settings_path(&paths),
            r#"{"theme":"dark","export":{"container":"avi","codec":"mpeg4","scale":1.0,"fps":null}}"#,
        )
        .unwrap();
        let loaded = AppSettings::load(&paths);
        assert_eq!(loaded.theme, AppTheme::Dark);
        assert_eq!(loaded.export, ExportSettings::default());
    }

    #[test]
    fn a_file_from_before_onboarding_has_no_record_and_asks_every_time() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        std::fs::write(app_settings_path(&paths), r#"{"theme":"dark"}"#).unwrap();
        let loaded = AppSettings::load(&paths);
        assert_eq!(loaded.theme, AppTheme::Dark);
        assert_eq!(loaded.onboarding_completed, None);
        assert_eq!(loaded.permission_policy, PermissionPolicy::AskEveryTime);
    }

    #[test]
    fn an_unknown_policy_is_the_default_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        std::fs::write(
            app_settings_path(&paths),
            r#"{"theme":"light","onboarding_completed":true,"permission_policy":"yolo"}"#,
        )
        .unwrap();
        let loaded = AppSettings::load(&paths);
        assert_eq!(loaded.theme, AppTheme::Light);
        assert_eq!(loaded.onboarding_completed, Some(true));
        assert_eq!(loaded.permission_policy, PermissionPolicy::AskEveryTime);

        let chosen = AppSettings {
            onboarding_completed: Some(true),
            permission_policy: PermissionPolicy::AllowInWorkingCopy,
            ..AppSettings::default()
        };
        chosen.save(&paths).unwrap();
        assert_eq!(AppSettings::load(&paths), chosen);
    }

    #[test]
    fn the_motion_preference_round_trips_and_older_files_read_as_system() {
        let without: AppSettings =
            serde_json::from_str(r#"{"theme":"dark","onboarding_completed":true}"#).unwrap();
        assert_eq!(without.motion, MotionPreference::System);
        assert_eq!(without.theme, AppTheme::Dark);
        assert_eq!(without.onboarding_completed, Some(true));

        let unknown: AppSettings =
            serde_json::from_str(r#"{"theme":"light","motion":"wobbly"}"#).unwrap();
        assert_eq!(unknown.motion, MotionPreference::System);
        assert_eq!(unknown.theme, AppTheme::Light);

        for motion in [
            MotionPreference::System,
            MotionPreference::Reduced,
            MotionPreference::Full,
        ] {
            let settings = AppSettings {
                theme: AppTheme::Dark,
                motion,
                ..AppSettings::default()
            };
            let text = serde_json::to_string(&settings).unwrap();
            assert!(text.contains("\"motion\""), "{text}");
            assert_eq!(
                serde_json::from_str::<AppSettings>(&text).unwrap(),
                settings
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path()).unwrap();
        let reduced = AppSettings {
            motion: MotionPreference::Reduced,
            ..AppSettings::default()
        };
        reduced.save(&paths).unwrap();
        assert_eq!(AppSettings::load(&paths), reduced);
    }

    #[test]
    fn onboarding_opens_until_recorded_unless_the_install_is_already_set_up() {
        let unrecorded = AppSettings::default();
        assert!(needs_onboarding(&unrecorded, false, false));
        assert!(needs_onboarding(&unrecorded, true, false));
        assert!(needs_onboarding(&unrecorded, false, true));
        assert!(!needs_onboarding(&unrecorded, true, true));
        let quit_midway = AppSettings {
            onboarding_completed: Some(false),
            ..AppSettings::default()
        };
        assert!(needs_onboarding(&quit_midway, true, true));
        let done = AppSettings {
            onboarding_completed: Some(true),
            ..AppSettings::default()
        };
        assert!(!needs_onboarding(&done, false, false));
    }
}
