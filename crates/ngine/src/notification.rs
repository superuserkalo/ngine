use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bar;
use crate::engine::Problem;
use crate::format;

pub const APP_NAME: &str = "ngine";
pub const APP_ICON: &str = "ngine";
pub const OPEN: &str = "default";
pub const SHOW_FOLDER: &str = "folder";
const SAVED_FOR_MS: i32 = 10_000;
const PROBLEM_FOR_MS: i32 = 5_000;
const OPENER: &str = "xdg-open";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgency {
    Normal,
    Critical,
}

impl Urgency {
    fn byte(self) -> u8 {
        match self {
            Self::Normal => 1,
            Self::Critical => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hint {
    Byte(u8),
    Text(String),
}

/// A desktop notification, in the terms of the freedesktop notification
/// specification, with the hints Omarchy's own shell reads on top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub expire_ms: i32,
    pub glyph: &'static str,
    pub file: Option<PathBuf>,
}

fn json_list(words: &[String]) -> String {
    let quoted: Vec<String> = words.iter().map(|word| bar::quoted(word)).collect();
    format!("[{}]", quoted.join(","))
}

impl Notification {
    fn saved(kind: &str, file: &Path, length: Duration) -> Self {
        Self {
            summary: format!("{kind} saved"),
            body: format!(
                "{} · {}",
                format::length(length),
                file.file_name().map_or_else(
                    || file.display().to_string(),
                    |name| name.to_string_lossy().into_owned()
                )
            ),
            urgency: Urgency::Normal,
            expire_ms: SAVED_FOR_MS,
            glyph: bar::RECORDING_ICON,
            file: Some(file.to_owned()),
        }
    }

    pub fn recording_saved(file: &Path, length: Duration) -> Self {
        Self::saved("Recording", file, length)
    }

    pub fn replay_saved(file: &Path, length: Duration) -> Self {
        Self {
            glyph: bar::REPLAY_ICON,
            ..Self::saved("Replay", file, length)
        }
    }

    pub fn problem(problem: &Problem) -> Self {
        let body = match problem {
            Problem::DiskFull { folder } => format!("Free some space in {}", folder.display()),
            Problem::CaptureLost | Problem::EncoderFailed | Problem::WriteFailed => {
                "What was recorded until now is kept".into()
            }
            Problem::PermissionMissing(_) | Problem::CaptureRestarted => String::new(),
        };
        Self {
            summary: problem.summary().into(),
            body,
            urgency: Urgency::Critical,
            expire_ms: PROBLEM_FOR_MS,
            glyph: "\u{f0026}",
            file: None,
        }
    }

    fn folder(&self) -> Option<&Path> {
        self.file.as_deref().and_then(Path::parent)
    }

    /// The program and arguments that carry out an action the person chose.
    pub fn command_for(&self, action: &str) -> Option<Vec<String>> {
        let target = match action {
            OPEN => self.file.as_deref()?,
            SHOW_FOLDER => self.folder()?,
            _ => return None,
        };
        Some(vec![OPENER.into(), target.to_string_lossy().into_owned()])
    }

    /// The `actions` argument of `Notify`: identifiers and labels in turn.
    pub fn actions(&self) -> Vec<String> {
        let mut actions = Vec::new();
        if self.file.is_some() {
            actions.extend([OPEN.into(), "Open".into()]);
        }
        if self.folder().is_some() {
            actions.extend([SHOW_FOLDER.into(), "Show in folder".into()]);
        }
        actions
    }

    /// The `hints` argument of `Notify`. Omarchy 4's shell runs
    /// `omarchy-exec-argv` on a click and shows `omarchy-actions` as buttons
    /// itself; other daemons ignore them and send `ActionInvoked` instead.
    pub fn hints(&self) -> Vec<(&'static str, Hint)> {
        let mut hints = vec![
            ("urgency", Hint::Byte(self.urgency.byte())),
            ("desktop-entry", Hint::Text(APP_NAME.into())),
            ("omarchy-glyph", Hint::Text(self.glyph.into())),
        ];
        if self.file.is_some() {
            hints.push(("category", Hint::Text("transfer.complete".into())));
        }
        let buttons: Vec<(String, Vec<String>)> = self
            .actions()
            .chunks(2)
            .filter_map(|pair| match pair {
                [id, label] => Some((label.clone(), self.command_for(id)?)),
                _ => None,
            })
            .collect();
        if let Some((_, first)) = buttons.first() {
            hints.push(("omarchy-exec-argv", Hint::Text(json_list(first))));
            let described: Vec<String> = buttons
                .iter()
                .map(|(label, argv)| {
                    format!(
                        "{{\"label\":{},\"argv\":{}}}",
                        bar::quoted(label),
                        json_list(argv)
                    )
                })
                .collect();
            hints.push((
                "omarchy-actions",
                Hint::Text(format!("[{}]", described.join(","))),
            ));
        }
        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Permission;

    fn hint(notification: &Notification, key: &str) -> Option<Hint> {
        notification
            .hints()
            .into_iter()
            .find(|(name, _)| *name == key)
            .map(|(_, hint)| hint)
    }

    fn text(value: &str) -> Hint {
        Hint::Text(value.into())
    }

    #[test]
    fn a_saved_recording_offers_to_open_the_file_and_its_folder() {
        let file = Path::new("/home/me/Videos/ngine/Recording 2026-11-16 at 10.00.00.mp4");
        let saved = Notification::recording_saved(file, Duration::from_secs(67));
        assert_eq!(saved.summary, "Recording saved");
        assert_eq!(
            saved.body,
            "1 min 7 s · Recording 2026-11-16 at 10.00.00.mp4"
        );
        assert_eq!(saved.expire_ms, 10_000);
        assert_eq!(
            saved.actions(),
            ["default", "Open", "folder", "Show in folder"]
        );
        assert_eq!(
            saved.command_for(OPEN),
            Some(vec!["xdg-open".into(), file.display().to_string()])
        );
        assert_eq!(
            saved.command_for(SHOW_FOLDER),
            Some(vec!["xdg-open".into(), "/home/me/Videos/ngine".into()])
        );
        assert_eq!(saved.command_for("dance"), None);
    }

    #[test]
    fn the_hints_carry_urgency_identity_and_omarchy_buttons() {
        let file = Path::new("/v/Replay \"1\".mp4");
        let saved = Notification::replay_saved(file, Duration::from_secs(30));
        assert_eq!(saved.summary, "Replay saved");
        assert_eq!(hint(&saved, "urgency"), Some(Hint::Byte(1)));
        assert_eq!(hint(&saved, "desktop-entry"), Some(text("ngine")));
        assert_eq!(hint(&saved, "category"), Some(text("transfer.complete")));
        assert_eq!(
            hint(&saved, "omarchy-exec-argv"),
            Some(text(r#"["xdg-open","/v/Replay \"1\".mp4"]"#))
        );
        assert_eq!(
            hint(&saved, "omarchy-actions"),
            Some(text(
                r#"[{"label":"Open","argv":["xdg-open","/v/Replay \"1\".mp4"]},{"label":"Show in folder","argv":["xdg-open","/v"]}]"#
            ))
        );
    }

    #[test]
    fn a_problem_is_critical_and_offers_nothing_to_open() {
        let full = Notification::problem(&Problem::DiskFull {
            folder: PathBuf::from("/clips"),
        });
        assert_eq!(full.summary, "The disk is full");
        assert_eq!(full.body, "Free some space in /clips");
        assert_eq!(hint(&full, "urgency"), Some(Hint::Byte(2)));
        assert_eq!(full.actions(), Vec::<String>::new());
        assert_eq!(hint(&full, "omarchy-exec-argv"), None);
        let denied = Notification::problem(&Problem::PermissionMissing(Permission::Microphone));
        assert_eq!(denied.summary, "ngine is not allowed to use the microphone");
    }
}
