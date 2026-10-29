//! Identities shared by the studio engine and its UI.
//!
//! Every event the engine sends about an open project carries the [`SessionId`] of the
//! project session it came from, so a window drops what a closed project still says. A
//! [`CommandId`] pairs a command with the event that reports its completion.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One opened project, from open until it is closed or replaced. Reopening the folder that
/// is already open keeps its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// One command sent to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(pub u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session {}", self.0)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command {}", self.0)
    }
}

/// The appearance the user chose. `System` follows the operating system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppTheme {
    #[default]
    System,
    Light,
    Dark,
}

/// How much the studio animates, chosen by the user. `System` follows the operating system
/// where the UI toolkit reports it, and is full motion otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MotionPreference {
    #[default]
    System,
    /// Animations snap to their end state, spinners are static.
    Reduced,
    Full,
}

/// How agent permission requests are answered, chosen by the user (onboarding or Settings).
/// Studio answers a request itself only when this policy covers it; everything else is a
/// card the user answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Every request is a card (Studio never answers one).
    #[default]
    AskEveryTime,
    /// Reads, searches and edits whose every location is inside the task's working copy
    /// are allowed once; commands, network access and anything else ask.
    AllowInWorkingCopy,
    /// Every request is allowed once, commands and network access included.
    AllowEverything,
}

impl PermissionPolicy {
    pub const ALL: [PermissionPolicy; 3] = [
        PermissionPolicy::AskEveryTime,
        PermissionPolicy::AllowInWorkingCopy,
        PermissionPolicy::AllowEverything,
    ];

    /// The short name the UI shows for the policy.
    pub fn label(self) -> &'static str {
        match self {
            PermissionPolicy::AskEveryTime => "Ask every time",
            PermissionPolicy::AllowInWorkingCopy => "Allow in the working copy",
            PermissionPolicy::AllowEverything => "Allow everything",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_policy_is_stored_by_snake_case_name() {
        for (policy, name) in [
            (PermissionPolicy::AskEveryTime, "\"ask_every_time\""),
            (
                PermissionPolicy::AllowInWorkingCopy,
                "\"allow_in_working_copy\"",
            ),
            (PermissionPolicy::AllowEverything, "\"allow_everything\""),
        ] {
            assert_eq!(serde_json::to_string(&policy).unwrap(), name);
            assert_eq!(
                serde_json::from_str::<PermissionPolicy>(name).unwrap(),
                policy
            );
        }
        assert_eq!(PermissionPolicy::default(), PermissionPolicy::AskEveryTime);
    }

    #[test]
    fn app_theme_is_stored_by_lowercase_name() {
        for (theme, name) in [
            (AppTheme::System, "\"system\""),
            (AppTheme::Light, "\"light\""),
            (AppTheme::Dark, "\"dark\""),
        ] {
            assert_eq!(serde_json::to_string(&theme).unwrap(), name);
            assert_eq!(serde_json::from_str::<AppTheme>(name).unwrap(), theme);
        }
        assert_eq!(AppTheme::default(), AppTheme::System);
    }

    #[test]
    fn motion_preference_is_stored_by_lowercase_name() {
        for (motion, name) in [
            (MotionPreference::System, "\"system\""),
            (MotionPreference::Reduced, "\"reduced\""),
            (MotionPreference::Full, "\"full\""),
        ] {
            assert_eq!(serde_json::to_string(&motion).unwrap(), name);
            assert_eq!(
                serde_json::from_str::<MotionPreference>(name).unwrap(),
                motion
            );
        }
        assert_eq!(MotionPreference::default(), MotionPreference::System);
    }
}
