use std::path::Path;

use super::{Autostart, BACKGROUND_FLAG, Folders};

pub const VIDEOS_FOLDER: &str = "Movies";
pub const LISTS_CAPTURE_TARGETS: bool = true;

const AGENT_FILE: &str = "Library/LaunchAgents/ngine.plist";

fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
