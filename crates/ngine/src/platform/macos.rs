use std::path::Path;
use std::process::Command;

use super::{Autostart, BACKGROUND_FLAG, Folders};
use crate::engine::Permission;

pub const VIDEOS_FOLDER: &str = "Movies";
pub const SAMPLE_OUTPUT_FOLDER: &str = "~/Movies/ngine";
pub const SAMPLE_BUFFER_FOLDER: &str = "~/Library/Caches/ngine/replay";
pub const FILE_MANAGER: &str = "Finder";
pub const TRASH: &str = "Trash";
pub const SYSTEM_SETTINGS: &str = "System Settings";
pub const LISTS_CAPTURE_TARGETS: bool = true;
pub const ERASE_KEY: &str = "Delete";

/// Keys macOS keeps for itself or that every Mac app answers, with what they
/// do there.
pub const RESERVED_SHORTCUTS: &[(&str, &str)] = &[
    ("cmd+q", "quitting an app"),
    ("cmd+w", "closing a window"),
    ("cmd+h", "hiding an app"),
    ("cmd+m", "minimising a window"),
    ("cmd+tab", "switching apps"),
    ("cmd+backquote", "switching windows"),
    ("cmd+space", "Spotlight"),
    ("ctrl+space", "switching input sources"),
    ("cmd+alt+escape", "Force Quit"),
    ("cmd+shift+3", "the screenshot of the screen"),
    ("cmd+shift+4", "the screenshot of a part"),
    ("cmd+shift+5", "the Screenshot app"),
    ("ctrl+cmd+q", "locking the screen"),
    ("ctrl+left", "the Space to the left"),
    ("ctrl+right", "the Space to the right"),
    ("ctrl+up", "Mission Control"),
    ("ctrl+down", "App Exposé"),
];

const AGENT_FILE: &str = "Library/LaunchAgents/ngine.plist";

fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The window keeps the system's own traffic lights over a content view that
/// fills it, and only the strips ngine marks move it. A titlebar ngine drew
/// itself would have Rune move the lights, which macOS 26 refuses.
pub fn window_frame<S>(app: rune::App<S>) -> rune::App<S> {
    app.chrome(rune::Chrome {
        full_size_content: true,
        traffic_lights: None,
        drag_regions: true,
    })
}

pub fn reveal(file: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg("-R").arg(file);
    command
}

pub fn open_folder(folder: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(folder);
    command
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "other systems have no such page, and the callers ask every system alike"
)]
pub fn privacy_settings(permission: Permission) -> Option<Command> {
    let pane = match permission {
        Permission::ScreenRecording => "Privacy_ScreenCapture",
        Permission::Microphone => "Privacy_Microphone",
    };
    let mut command = Command::new("open");
    command.arg(format!(
        "x-apple.systempreferences:com.apple.preference.security?{pane}"
    ));
    Some(command)
}

pub fn autostart(folders: &Folders<'_>, program: &Path) -> Autostart {
    let program = xml_text(&program.to_string_lossy());
    Autostart {
        file: folders.home.join(AGENT_FILE),
        contents: format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ngine</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>{BACKGROUND_FLAG}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
        ),
    }
}
