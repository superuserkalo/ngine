use rune::tok::{radius, space, type_};
use rune::{
    ChromeInsets, Color, Cx, El, FileDialogOptions, Icon, ScrollSpec, Titlebar, Tx, col, el, icon,
    row, spacer, text,
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

use widgets::{Button, HEADING, ICON, Look, NUMBERS, Paint, STRONG, caption};

pub const EARLY_NOTE: &str = "Early build. So far it has only been tested on macOS.";
pub const SIMULATED_NOTE: &str =
    "Capture is simulated on this system, so nothing is written to disk.";

const SIDEBAR_WIDTH: f32 = 184.0;
const RAIL_WIDTH: f32 = 56.0;
const NAV_HEIGHT: f32 = 32.0;
const DOT: f32 = 6.0;
pub const TITLEBAR: f32 = 40.0;
pub const PAGE_PADDING: f32 = 24.0;
pub const NARROW_BELOW: f32 = 900.0;
const WIDEST_CONTENT: f32 = 920.0;

/// What every screen needs to draw itself: the colours, the app, how much
/// room the window gives, and the layer that floats above the screens.
pub struct Ui<'a> {
    pub p: Paint,
    pub app: &'a Ngine,
    pub layer: Vec<El>,
    pub window: (f32, f32),
    pub band: f32,
}

/// The titlebar ngine draws in place of the system's: the window's own
/// controls sit over it, and its empty parts move the window.
#[cfg_attr(
    target_os = "macos",
    expect(
        dead_code,
        reason = "macOS keeps its own traffic lights over a full-size content view"
    )
)]
pub fn titlebar() -> Titlebar {
    Titlebar::new(TITLEBAR)
}

/// How tall the strip across the top is that the window's own controls sit
/// in and that moves the window: the titlebar ngine draws, or, where the
/// system draws its buttons over the content, a strip as tall.
fn top_band(chrome: ChromeInsets, under_window_buttons: bool) -> f32 {
    if chrome.height > 0.0 {
        chrome.height
    } else if under_window_buttons {
        TITLEBAR
    } else {
        0.0
    }
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
    let top = if ui.band > 0.0 {
        space::S1
    } else {
        PAGE_PADDING
    };
    col()
        .key(key)
        .w(ui.content_width())
        .self_center()
        .pt(top)
        .pb(PAGE_PADDING)
        .gap(space::S6)
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
    let live = (screen == Screen::Capture).then(|| live_mark(ui)).flatten();
    if ui.narrow() {
        item = item.center().help(label);
        if let Some((_, words, colour)) = live {
            item = item.help(format!("{label} · {words}")).child(
                el().key(format!("{key}-live"))
                    .size(DOT, DOT)
                    .radius(DOT / 2.0)
                    .fill(colour)
                    .absolute(RAIL_WIDTH / 2.0 + 4.0, 7.0)
                    .a11y_hidden(),
            );
        }
    } else {
        item = item
            .px(space::S2 + 2.0)
            .child(text(label).font(type_::LABEL).color(ink).a11y_hidden());
        if let Some((mark, words, _)) = live {
            item = item
                .label(format!("{label}, {words}"))
                .child(spacer())
                .child(mark);
        }
    }
    item
}

/// What the Capture item shows while something is live: a red dot and the
/// time while recording, a quiet mark while instant replay holds the past.
fn live_mark(ui: &Ui<'_>) -> Option<(El, String, Color)> {
    let p = &ui.p;
    let status = &ui.app.status;
    if let Some(recording) = &status.recording {
        let time = format::timer(recording.elapsed);
        let mark = row()
            .key("live-recording")
            .gap(space::S1 + 2.0)
            .center_y()
            .child(
                el().key("live-recording-dot")
                    .size(DOT, DOT)
                    .radius(DOT / 2.0)
                    .fill(p.danger)
                    .a11y_hidden(),
            )
            .child(
                text(time.clone())
                    .key("live-recording-time")
                    .font(NUMBERS)
                    .color(p.t.text_secondary)
                    .a11y_hidden(),
            );
        return Some((mark, format!("recording {time}"), p.danger));
    }
    status.replay.as_ref().map(|replay| {
        let words = format!("instant replay holds {}", format::length(replay.held));
        let mark = icon(&icons::HISTORY, 13.0)
            .key("live-replay")
            .color(p.accent)
            .help(format!(
                "Instant replay is on and holds {}",
                format::length(replay.held)
            ))
            .a11y_hidden();
        (mark, words, p.accent)
    })
}

fn settings_item(ui: &Ui<'_>) -> El {
    let p = &ui.p;
    let open = ui.app.settings_open;
    let ink = if open { p.t.text } else { p.t.text_secondary };
    let mut item = row()
        .key("nav-settings")
        .role("button")
        .label("Settings")
        .expanded(open)
        .h(NAV_HEIGHT)
        .gap(space::S2 + 2.0)
        .center_y()
        .radius(radius::MD)
        .hover(p.t.text.alpha(0.04))
        .press(p.t.press)
        .focusable(true)
        .on_press(Ngine::open_settings)
        .child(icon(&icons::SETTINGS, 16.0).color(ink).a11y_hidden());
    if ui.narrow() {
        item = item.center().help("Settings");
    } else {
        item = item
            .px(space::S2 + 2.0)
            .child(text("Settings").font(type_::LABEL).color(ink).a11y_hidden());
    }
    item
}

fn sidebar(ui: &Ui<'_>) -> El {
    let p = &ui.p;
    let narrow = ui.narrow();
    let titlebar_height = ui.band;
    let top = if titlebar_height > 0.0 {
        titlebar_height
    } else {
        space::S3
    };
    let mut brand = row().key("brand").h(28.0).center_y();
    brand = if narrow {
        brand
    } else {
        brand
            .px(space::S2 + 2.0)
            .child(text(NAME).key("name").font(HEADING).color(p.t.text))
    };
    col()
        .key("sidebar")
        .role("navigation")
        .label("Sections")
        .w(if narrow { RAIL_WIDTH } else { SIDEBAR_WIDTH })
        .h_full()
        .px(space::S2)
        .pb(space::S3)
        .pt(top)
        .gap(2.0)
        .fill(p.sidebar)
        .border_sides([0.0, p.hair, 0.0, 0.0], p.t.separator)
        .window_drag()
        .child(brand)
        .child(el().key("brand-gap").h(space::S3).a11y_hidden())
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
        .child(settings_item(ui))
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
            .look(Look::Plain)
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
                    .look(Look::Plain)
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
                    .look(Look::Plain)
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
                    .look(Look::Plain)
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
                    .look(Look::Plain)
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

fn banners(p: &Paint, app: &Ngine, width: f32) -> Option<El> {
    let shown = if let Some(problem) = app.problems.first() {
        let mut one = problem_banner(p, problem);
        let more = app.problems.len() - 1 + usize::from(app.settings_were_reset);
        if more > 0 {
            one.detail = match more {
                1 => format!("{} One more problem after this one.", one.detail),
                many => format!("{} {many} more problems after this one.", one.detail),
            };
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
                    app.open_settings();
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
            .w(width)
            .self_center()
            .pt(space::S1)
            .pb(space::S3)
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
    cx.caption_color(p.t.text);
    let window = cx
        .layout_size("window")
        .map_or(WINDOW_SIZE, |size| (size[0], size[1]));
    let timeline = cx
        .layout_size("timeline")
        .map_or(editor::TIMELINE_WIDTH, |size| size[0]);
    let mut ui = Ui {
        p,
        app,
        layer: Vec::new(),
        window,
        band: top_band(cx.chrome_insets(), cx.titlebar_inset() > 0.0),
    };
    if app.watching()
        && let Some(watched) = editor::watched(&mut ui, timeline)
    {
        return col()
            .key("window")
            .role("window")
            .w_full()
            .h_full()
            .fill(Color::BLACK)
            .child(watched)
            .children(ui.layer);
    }
    let screen = match app.screen {
        Screen::Capture => capture::view(&mut ui),
        Screen::Library => library::view(&mut ui),
        Screen::Editor => editor::view(&mut ui, timeline),
    };
    let mut main = col().key("main").grow(1.0).h_full().min_w(0.0).min_h(0.0);
    if ui.band > 0.0 {
        main = main.child(
            el().key("main-titlebar")
                .w_full()
                .h(ui.band)
                .window_drag()
                .a11y_hidden(),
        );
    }
    if let Some(banners) = banners(&p, app, ui.content_width()) {
        main = main.child(banners);
    }
    main = main.child(screen);
    if let Some(notice) = notice(&p, app) {
        main = main.child(notice);
    }
    if app.settings_open {
        settings::sheet(&mut ui);
    }
    row()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(sidebar(&ui))
        .child(main)
        .children(ui.layer)
}

#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod tests;
