#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const OMARCHY_3: &[(&str, &str)] = &[
    (
        ".config/hypr/hyprland.conf",
        "source = ~/.config/hypr/bindings.conf\n# Add any other personal Hyprland configuration below\n",
    ),
    (
        ".config/hypr/bindings.conf",
        "bindd = SUPER, RETURN, Terminal, exec, uwsm-app -- xdg-terminal-exec\n",
    ),
    (
        ".config/hypr/autostart.conf",
        "# Extra autostart processes\n# exec-once = uwsm-app -- my-service\n",
    ),
    (
        ".config/omarchy/extensions/menu.sh",
        "# Overwrite parts of the omarchy-menu with user-specific submenus.\n",
    ),
    (
        ".config/waybar/config.jsonc",
        "{\n  \"layer\": \"top\",\n  \"modules-center\": [\"clock\", \"custom/screenrecording-indicator\"],\n  \"clock\": {}\n}\n",
    ),
    (
        ".config/waybar/style.css",
        "#custom-screenrecording-indicator.active {\n  color: #a55555;\n}\n",
    ),
];

const OMARCHY_4: &[(&str, &str)] = &[
    (
        ".config/hypr/hyprland.lua",
        "require(\"hypr.bindings\")\n-- o.window(\"qemu\", { workspace = \"5\" })\n",
    ),
    (
        ".config/hypr/bindings.lua",
        "-- o.bind(\"SUPER + H\", nil, \"voxtype record toggle\")\n",
    ),
    (
        ".config/hypr/autostart.lua",
        "-- Extra autostart processes.\n-- o.launch_on_start(\"my-service\")\n",
    ),
    (
        ".config/omarchy/extensions/omarchy-menu.jsonc",
        "{\n  // Extend the Quickshell Omarchy menu with JSONC.\n}\n",
    ),
];

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/omarchy")
}

fn fake_home(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let home = std::env::temp_dir().join(format!("ngine-omarchy-{name}-{}", std::process::id()));
    if home.exists() {
        fs::remove_dir_all(&home).expect("an old fake home is removed");
    }
    for (path, contents) in files {
        let file = home.join(path);
        fs::create_dir_all(file.parent().expect("a folder")).expect("a folder is made");
        fs::write(file, contents).expect("a file is written");
    }
    home
}

fn tree(folder: &Path, root: &Path, found: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(folder).expect("a folder to read") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            tree(&path, root, found);
        } else {
            let name = path
                .strip_prefix(root)
                .expect("inside")
                .display()
                .to_string();
            found.insert(name, fs::read_to_string(&path).expect("a text file"));
        }
    }
}

fn snapshot(home: &Path) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    tree(home, home, &mut found);
    found
}

fn run(script: &str, home: &Path) {
    let output = Command::new("bash")
        .arg(scripts().join(script))
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env("NGINE_OMARCHY_NO_RELOAD", "1")
        .output()
        .expect("bash runs");
    assert!(
        output.status.success(),
        "{script} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn installs_once_and_uninstalls_cleanly(
    name: &str,
    files: &[(&str, &str)],
) -> BTreeMap<String, String> {
    let home = fake_home(name, files);
    let before = snapshot(&home);
    run("install.sh", &home);
    let installed = snapshot(&home);
    assert_ne!(installed, before);
    run("install.sh", &home);
    assert_eq!(
        snapshot(&home),
        installed,
        "a second install changes nothing"
    );
    run("uninstall.sh", &home);
    assert_eq!(snapshot(&home), before, "uninstall puts every file back");
    run("uninstall.sh", &home);
    assert_eq!(snapshot(&home), before);
    fs::remove_dir_all(&home).expect("the fake home is removed");
    installed
}

#[test]
fn omarchy_3_gets_bindings_menu_autostart_and_waybar_once() {
    let installed = installs_once_and_uninstalls_cleanly("3", OMARCHY_3);
    let file = |name: &str| installed.get(name).cloned().unwrap_or_default();
    assert!(
        file(".config/hypr/bindings.conf")
            .contains("bindd = ALT, PRINT, Screenrecording, exec, ngine record toggle")
    );
    assert!(
        file(".config/hypr/autostart.conf").contains("exec-once = uwsm-app -- ngine --background")
    );
    assert!(file(".config/omarchy/extensions/menu.sh").contains("show_screenrecord_menu()"));
    let waybar = file(".config/waybar/config.jsonc");
    assert!(
        waybar.contains("\"modules-center\": [\"custom/ngine\", \"clock\""),
        "{waybar}"
    );
    assert_eq!(waybar.matches("\"custom/ngine\": {").count(), 1, "{waybar}");
    assert!(file(".config/waybar/style.css").contains("#custom-ngine.recording"));
}

#[test]
fn omarchy_4_gets_bindings_menu_autostart_and_the_bar_plugin_once() {
    let installed = installs_once_and_uninstalls_cleanly("4", OMARCHY_4);
    let file = |name: &str| installed.get(name).cloned().unwrap_or_default();
    assert!(
        file(".config/hypr/bindings.lua")
            .contains("o.rebind(\"ALT + PRINT\", \"Screenrecording\", \"ngine record toggle\")")
    );
    assert!(
        file(".config/hypr/autostart.lua").contains("o.launch_on_start(\"ngine --background\")")
    );
    let menu = file(".config/omarchy/extensions/omarchy-menu.jsonc");
    assert!(menu.starts_with("{\n// ngine:begin\n"), "{menu}");
    assert!(menu.contains("\"action\":\"ngine record stop\""), "{menu}");
    assert!(installed.contains_key(".config/omarchy/plugins/ngine.status/manifest.json"));
    assert!(!installed.contains_key(".config/waybar/config.jsonc"));
}

#[test]
fn a_home_without_hyprland_is_refused() {
    let home = fake_home("none", &[(".config/other", "")]);
    let output = Command::new("bash")
        .arg(scripts().join("install.sh"))
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .env("NGINE_OMARCHY_NO_RELOAD", "1")
        .output()
        .expect("bash runs");
    assert!(!output.status.success());
    assert_eq!(snapshot(&home).len(), 1);
    fs::remove_dir_all(&home).expect("the fake home is removed");
}
