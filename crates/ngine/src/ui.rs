use rune::tok::{radius, space, type_};
use rune::{
    Color, Cx, El, FileDialogOptions, Icon, ScrollSpec, Tx, col, el, icon, row, spacer, text,
};

use crate::app::{Folder, NAME, Ngine, Screen, Section, WINDOW_SIZE};
use crate::engine::{Permission, Problem};
use crate::format;
use crate::platform;

mod capture;
mod editor;
mod icons;
mod library;
pub mod palette;
mod settings;
mod widgets;

use widgets::{Button, HEADING, ICON, Look, Paint, STRONG, TITLE, caption};

pub const EARLY_NOTE: &str = "Early build. So far it has only been tested on macOS.";
pub const SIMULATED_NOTE: &str =
    "Capture is simulated on this system, so nothing is written to disk.";

const SIDEBAR_WIDTH: f32 = 172.0;
const RAIL_WIDTH: f32 = 56.0;
const NAV_HEIGHT: f32 = 34.0;
const DOT: f32 = 8.0;
const WINDOW_BUTTONS_HEIGHT: f32 = 24.0;
pub const PAGE_PADDING: f32 = 20.0;
pub const NARROW_BELOW: f32 = 900.0;
const WIDEST_CONTENT: f32 = 920.0;

/// What every screen needs to draw itself: the colours, the app, how much
/// room the window gives, and the layer that floats above the screens.
pub struct Ui<'a> {
    pub p: Paint,
    pub app: &'a Ngine,
    pub layer: Vec<El>,
    pub window: (f32, f32),
}

impl Ui<'_> {
    pub fn narrow(&self) -> bool {
        self.window.0 < NARROW_BELOW
    }

    pub fn main_width(&self) -> f32 {
        let sidebar = if self.narrow() {
            RAIL_WIDTH
        } else {
            SIDEBAR_WIDTH
        };
        (self.window.0 - sidebar).max(1.0)
    }

    pub fn content_width(&self) -> f32 {
        (self.main_width() - 2.0 * PAGE_PADDING).clamp(1.0, WIDEST_CONTENT)
    }
}

/// The column a screen lays its content in: padded, and no wider than reads
/// well on a large monitor.
pub fn sheet(ui: &Ui<'_>, key: &str) -> El {
    col()
        .key(key)
        .w(ui.content_width())
        .self_center()
        .py(PAGE_PADDING)
        .gap(space::S6 - 4.0)
}

/// The keys of the hotkey in `slot`, when it is set and works.
pub fn hotkey_caps(app: &Ngine, slot: usize) -> Option<Vec<String>> {
    if app.hotkey_problems[slot].is_some() {
        return None;
    }
    crate::hotkeys::keycaps(app.settings.hotkeys.all()[slot])
}

/// Asks the system for a folder and puts the answer into the settings.
pub fn choose_folder(folder: Folder) -> impl Fn(&mut Ngine, &mut Tx) + Clone + 'static {
    move |app: &mut Ngine, tx: &mut Tx| {
        let (title, current) = match folder {
            Folder::Output => (
                "Choose where recordings are saved",
                app.settings.output_folder.clone(),
            ),
            Folder::Buffer => (
                "Choose where the instant replay buffer is kept",
                app.settings.replay.buffer_folder.clone(),
            ),
        };
        let options = FileDialogOptions {
            title: title.into(),
            directory: current.is_dir().then_some(current),
            ..FileDialogOptions::default()
        };
        tx.open_folder(options, move |app: &mut Ngine, answer, _| {
            app.folder_chosen(folder, answer.map_err(|error| error.to_string()));
        });
    }
}

pub fn page(key: &str, content: El) -> El {
    col()
        .key(format!("{key}-scroll"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(content)
}

pub fn page_title(p: &Paint, key: &str, title: &str) -> El {
    text(title)
        .key(key)
        .role("heading")
        .level(1)
        .font(TITLE)
        .color(p.t.text)
}

fn nav_item(ui: &Ui<'_>, screen: Screen, key: &str, label: &str, glyph: &'static Icon) -> El {
    let p = &ui.p;
    let shown = match ui.app.screen {
        Screen::Editor => Screen::Library,
        other => other,
    };
    let current = shown == screen;
    let ink = if current {
        p.t.text
    } else {
        p.t.text_secondary
    };
    let mut item = row()
        .key(key)
        .role("button")
        .label(label)
        .checked(current)
        .h(NAV_HEIGHT)
        .gap(space::S2 + 2.0)
        .center_y()
        .radius(radius::MD)
        .fill(if current {
            p.t.control_off
        } else {
            Color::TRANSPARENT
        })
        .hover(p.t.text.alpha(0.04))
        .press(p.t.press)
        .focusable(true)
        .on_press(move |app: &mut Ngine| app.show(screen))
        .child(icon(glyph, 16.0).color(ink).a11y_hidden());
    if ui.narrow() {
        item = item.center().help(label);
    } else {
        item = item
            .px(space::S3 - 2.0)
            .child(text(label).font(type_::LABEL).color(ink).a11y_hidden());
    }
    item
}

fn status_words(app: &Ngine) -> Vec<(&'static str, bool, String)> {
    let mut lines = Vec::new();
    if let Some(recording) = &app.status.recording {
        lines.push((
            "status-recording",
            true,
            format!("Recording {}", format::timer(recording.elapsed)),
        ));
    }
    if let Some(replay) = &app.status.replay {
        lines.push((
            "status-replay",
            false,
            format!("Replay on · {}", format::length(replay.length)),
        ));
    }
    lines
}

fn status(ui: &Ui<'_>) -> El {
    let p = &ui.p;
    let lines = status_words(ui.app);
    let spoken = if lines.is_empty() {
        "Ready".to_owned()
    } else {
        lines
            .iter()
            .map(|(_, _, words)| words.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let line = |key: &str, colour: Color, words: String| {
        let mut one = row().key(key).gap(space::S2).center_y().child(
            el().key(format!("{key}-dot"))
                .size(DOT, DOT)
                .radius(DOT / 2.0)
                .fill(colour)
                .a11y_hidden(),
        );
        if !ui.narrow() {
            one = one.child(
                text(words)
                    .key(format!("{key}-text"))
                    .font(type_::LABEL.features("tnum"))
                    .color(p.t.text)
                    .a11y_hidden(),
            );
        }
        one
    };
    let mut block = col()
        .key("status")
        .role("button")
        .label(spoken.clone())
        .help(format!("{spoken}. Go to Capture"))
        .gap(space::S2)
        .py(space::S2 + 2.0)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .hover(p.t.text.alpha(0.04))
        .press(p.t.press)
        .focusable(true)
        .on_press(|app: &mut Ngine| app.show(Screen::Capture));
    block = if ui.narrow() {
        block.center()
    } else {
        block.px(space::S3)
    };
    if lines.is_empty() {
        block = block.child(line("status-idle", p.good.alpha(0.8), "Ready".into()));
    }
    for (key, recording, words) in lines {
        let colour = if recording { p.danger } else { p.accent };
        block = block.child(line(key, colour, words));
    }
    block
}

fn sidebar(ui: &Ui<'_>, under_window_buttons: bool) -> El {
    let p = &ui.p;
    let narrow = ui.narrow();
    let top = if under_window_buttons {
        space::S3 + WINDOW_BUTTONS_HEIGHT
    } else {
        space::S3
    };
    let mut brand = row().key("brand").h(28.0).center_y();
    brand = if narrow {
        brand.center().child(
            text("n")
                .key("name")
                .font(HEADING)
                .color(p.t.text)
                .a11y_hidden(),
        )
    } else {
        brand
            .px(space::S3 - 2.0)
            .child(text(NAME).key("name").font(HEADING).color(p.t.text))
    };
    col()
        .key("sidebar")
        .role("navigation")
        .label("Sections")
        .w(if narrow { RAIL_WIDTH } else { SIDEBAR_WIDTH })
        .h_full()
        .px(space::S2 + 2.0)
        .pb(space::S2 + 2.0)
        .pt(top)
        .gap(space::S1)
        .fill(p.sidebar)
        .border_sides([0.0, p.hair, 0.0, 0.0], p.t.separator)
        .child(brand)
        .child(el().key("brand-gap").h(space::S2).a11y_hidden())
        .child(nav_item(
            ui,
            Screen::Capture,
            "nav-capture",
            "Capture",
            &icons::VIDEO,
        ))
        .child(nav_item(
            ui,
            Screen::Library,
            "nav-library",
            "Library",
            &icons::IMAGES,
        ))
        .child(spacer())
        .child(nav_item(
            ui,
            Screen::Settings,
            "nav-settings",
            "Settings",
            &icons::SETTINGS,
        ))
        .child(el().key("status-gap").h(space::S1).a11y_hidden())
        .child(status(ui))
}

struct Banner {
    key: &'static str,
    title: &'static str,
    detail: String,
    serious: bool,
    action: Option<El>,
}

fn open_settings_button(p: &Paint, key: &str, permission: Permission) -> Option<El> {
    platform::has_privacy_settings(permission).then(|| {
        Button::new(key, &format!("Open {}", platform::SYSTEM_SETTINGS))
            .look(Look::Primary)
            .build(p, move |app| app.open_privacy_settings(permission))
    })
}

fn problem_banner(p: &Paint, problem: &Problem) -> Banner {
    let title = problem.summary();
    match problem {
        Problem::PermissionMissing(Permission::ScreenRecording) => Banner {
            key: "problem-permission-screen",
            title,
            detail: format!(
                "Allow ngine under Screen Recording in {}, then start again.",
                platform::SYSTEM_SETTINGS
            ),
            serious: true,
            action: open_settings_button(
                p,
                "problem-permission-screen-action",
                Permission::ScreenRecording,
            ),
        },
        Problem::PermissionMissing(Permission::Microphone) => Banner {
            key: "problem-permission-microphone",
            title,
            detail: format!(
                "Allow ngine under Microphone in {}, or switch the microphone off on the Capture screen.",
                platform::SYSTEM_SETTINGS
            ),
            serious: true,
            action: open_settings_button(
                p,
                "problem-permission-microphone-action",
                Permission::Microphone,
            ),
        },
        Problem::CaptureRestarted => Banner {
            key: "problem-capture-restarted",
            title,
            detail: "Everything recorded before that is kept.".into(),
            serious: false,
            action: None,
        },
        Problem::CaptureLost => Banner {
            key: "problem-capture-lost",
            title,
            detail: "What was recorded up to then is saved.".into(),
            serious: true,
            action: Some(
                Button::new("problem-capture-lost-action", "Retry")
                    .look(Look::Primary)
                    .build(p, Ngine::retry_capture),
            ),
        },
        Problem::WriteFailed => Banner {
            key: "problem-write-failed",
            title,
            detail: "What was written up to then is kept.".into(),
            serious: true,
            action: Some(
                Button::new("problem-write-failed-action", "Choose another folder")
                    .look(Look::Primary)
                    .build_tx(p, choose_folder(Folder::Output)),
            ),
        },
        Problem::EncoderFailed => Banner {
            key: "problem-encoder-failed",
            title,
            detail: "What was recorded up to then is saved. Another codec may work.".into(),
            serious: true,
            action: Some(
                Button::new("problem-encoder-failed-action", "Open video settings")
                    .look(Look::Primary)
                    .build(p, |app| app.show_section(Section::Video)),
            ),
        },
        Problem::DiskFull { folder } => Banner {
            key: "problem-disk-full",
            title,
            detail: format!(
                "What was recorded up to then is saved. Free some space for {}, or save somewhere else.",
                folder.display()
            ),
            serious: true,
            action: Some(
                Button::new("problem-disk-full-action", "Choose another folder")
                    .look(Look::Primary)
                    .build_tx(p, choose_folder(Folder::Output)),
            ),
        },
    }
}

fn banner(p: &Paint, banner: Banner, dismiss: impl Fn(&mut Ngine) + 'static) -> El {
    let colour = if banner.serious { p.danger } else { p.warning };
    let glyph = if banner.serious {
        &icons::TRIANGLE_ALERT
    } else {
        &icons::INFO
    };
    let mut line = row()
        .key(banner.key)
        .role("alert")
        .label(banner.title)
        .pl(space::S3)
        .pr(space::S2)
        .py(space::S2)
        .gap(space::S3)
        .center_y()
        .radius(radius::LG)
        .fill(colour.alpha(0.12))
        .border(p.hair, colour.alpha(0.45))
        .child(icon(glyph, 18.0).color(colour).a11y_hidden())
        .child(
            col()
                .key(format!("{}-words", banner.key))
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(
                    text(banner.title)
                        .key(format!("{}-title", banner.key))
                        .font(STRONG)
                        .color(p.t.text)
                        .wrap(),
                )
                .child(caption(p, &format!("{}-detail", banner.key), banner.detail)),
        );
    if let Some(action) = banner.action {
        line = line.child(action);
    }
    line.child(
        Button::new(&format!("{}-dismiss", banner.key), "Dismiss")
            .only_icon(&icons::X)
            .look(Look::Quiet)
            .build(p, dismiss),
    )
}

fn banners(p: &Paint, app: &Ngine) -> Option<El> {
    let shown = if let Some(problem) = app.problems.first() {
        let mut one = problem_banner(p, problem);
        let more = app.problems.len() - 1 + usize::from(app.settings_were_reset);
        if more > 0 {
            one.detail = format!("{} {more} more after this one.", one.detail);
        }
        banner(p, one, |app| app.dismiss_problem(0))
    } else if app.settings_were_reset {
        let reset = Banner {
            key: "banner-settings-reset",
            title: "The settings file was damaged",
            detail: "ngine set it aside and went back to the default settings.".into(),
            serious: false,
            action: Some(
                Button::new("banner-settings-reset-action", "Review settings").build(p, |app| {
                    app.settings_were_reset = false;
                    app.show(Screen::Settings);
                }),
            ),
        };
        banner(p, reset, |app| app.settings_were_reset = false)
    } else {
        return None;
    };
    Some(
        col()
            .key("banners")
            .px(PAGE_PADDING)
            .pt(space::S3)
            .child(shown),
    )
}

fn notice(p: &Paint, app: &Ngine) -> Option<El> {
    let (words, colour, glyph) = match (&app.notice, &app.confirmation) {
        (Some(notice), _) if notice.is_error => {
            (notice.text.clone(), p.danger, &icons::TRIANGLE_ALERT)
        }
        (_, Some(confirmation)) => (confirmation.text.clone(), p.good, &icons::CHECK),
        (Some(notice), None) => (notice.text.clone(), p.t.text_secondary, &icons::INFO),
        (None, None) => return None,
    };
    Some(
        row()
            .key("notice-bar")
            .pin(Some(0.0), None, Some(0.0), Some(space::S3 + 2.0))
            .center()
            .child(
                row()
                    .key("notice-pill")
                    .max_w(520.0)
                    .min_h(32.0)
                    .px(space::S3)
                    .py(space::S1 + 2.0)
                    .gap(space::S2)
                    .center_y()
                    .radius(16.0)
                    .fill(p.raised)
                    .border(p.hair, p.outline)
                    .shadow(4.0, 16.0, p.t.shadow)
                    .child(icon(glyph, ICON).color(colour).a11y_hidden())
                    .child(
                        text(words)
                            .key("notice")
                            .role("status")
                            .font(type_::LABEL)
                            .color(p.t.text)
                            .wrap(),
                    ),
            ),
    )
}

pub fn view(app: &Ngine, cx: &mut Cx<Ngine>) -> El {
    let p = Paint::dark(cx.env().scale);
    cx.web_focus();
    cx.focus_ring(2.0, 1.5, p.accent);
    let window = cx
        .layout_size("window")
        .map_or(WINDOW_SIZE, |size| (size[0], size[1]));
    let timeline = cx
        .layout_size("timeline")
        .map_or(editor::TIMELINE_WIDTH, |size| size[0]);
    let under_window_buttons = cx.titlebar_inset() > 0.0;
    let mut ui = Ui {
        p,
        app,
        layer: Vec::new(),
        window,
    };
    let screen = match app.screen {
        Screen::Capture => capture::view(&mut ui),
        Screen::Settings => settings::view(&mut ui),
        Screen::Library => library::view(&mut ui),
        Screen::Editor => editor::view(&mut ui, timeline),
    };
    let mut main = col().key("main").grow(1.0).h_full().min_w(0.0).min_h(0.0);
    if let Some(banners) = banners(&p, app) {
        main = main.child(banners);
    }
    main = main.child(screen);
    if let Some(notice) = notice(&p, app) {
        main = main.child(notice);
    }
    row()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(sidebar(&ui, under_window_buttons))
        .child(main)
        .children(ui.layer)
}

#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod tests;
