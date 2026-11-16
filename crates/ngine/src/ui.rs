use rune::tok::{radius, space, type_};
use rune::{Cx, El, ScrollSpec, col, el, row, spacer, text};

use crate::app::{Action, NAME, Ngine, Screen, VERSION};
use crate::engine::{Permission, Problem};
use crate::format;

mod record;
mod settings;
mod widgets;

use widgets::{CAPTION, HEADING, Look, Paint, STRONG, button, caption};

pub const PREVIEW_NOTE: &str = "Preview build. Capture is simulated and no file is written yet.";

const SIDEBAR_WIDTH: f32 = 176.0;
const NAV_HEIGHT: f32 = 32.0;
const DOT: f32 = 8.0;
const CONTENT_PADDING: f32 = 20.0;
const SECTION_GAP: f32 = 16.0;
const PLACEHOLDER_HEIGHT: f32 = 420.0;
const WINDOW_BUTTONS_HEIGHT: f32 = 24.0;

const NAVIGATION: [(Screen, &str, &str); 4] = [
    (Screen::Record, "nav-record", "Record"),
    (Screen::Library, "nav-library", "Library"),
    (Screen::Editor, "nav-editor", "Clip editor"),
    (Screen::Settings, "nav-settings", "Settings"),
];

fn nav_item(p: &Paint, app: &Ngine, screen: Screen, key: &str, label: &str) -> El {
    let current = app.screen == screen;
    row()
        .key(key)
        .role("button")
        .label(label)
        .checked(current)
        .h(NAV_HEIGHT)
        .px(space::S3)
        .center_y()
        .radius(radius::MD)
        .fill(if current {
            p.t.control_off
        } else {
            rune::Color::TRANSPARENT
        })
        .hover(p.t.hover)
        .press(p.t.press)
        .focusable(true)
        .on_press(move |app: &mut Ngine| app.screen = screen)
        .child(
            text(label)
                .font(type_::LABEL)
                .color(if current {
                    p.t.text
                } else {
                    p.t.text_secondary
                })
                .a11y_hidden(),
        )
}

fn status_line(p: &Paint, key: &str, colour: rune::Color, words: String) -> El {
    row()
        .key(key)
        .role("status")
        .label(words.clone())
        .gap(space::S2)
        .center_y()
        .child(
            el().key(format!("{key}-dot"))
                .size(DOT, DOT)
                .radius(DOT / 2.0)
                .fill(colour)
                .a11y_hidden(),
        )
        .child(
            text(words)
                .key(format!("{key}-text"))
                .font(type_::LABEL)
                .color(p.t.text)
                .a11y_hidden(),
        )
}

fn status(p: &Paint, app: &Ngine) -> El {
    let mut lines = col().key("status").gap(space::S2).px(space::S3);
    if let Some(recording) = &app.status.recording {
        lines = lines.child(status_line(
            p,
            "status-recording",
            p.danger,
            format!("Recording {}", format::timer(recording.elapsed)),
        ));
    }
    if app.status.replay.is_some() {
        lines = lines.child(status_line(
            p,
            "status-replay",
            p.accent,
            "Replay on".into(),
        ));
    }
    if app.status.recording.is_none() && app.status.replay.is_none() {
        lines = lines.child(status_line(
            p,
            "status-idle",
            p.t.control_off,
            "Idle".into(),
        ));
    }
    lines
}

fn sidebar(p: &Paint, app: &Ngine, under_window_buttons: bool) -> El {
    let mut navigation = col().key("navigation").gap(2.0);
    for (screen, key, label) in NAVIGATION {
        navigation = navigation.child(nav_item(p, app, screen, key, label));
    }
    col()
        .key("sidebar")
        .w(SIDEBAR_WIDTH)
        .h_full()
        .padding(space::S3)
        .pt(if under_window_buttons {
            space::S3 + WINDOW_BUTTONS_HEIGHT
        } else {
            space::S3
        })
        .gap(space::S3)
        .fill(p.t.card)
        .child(
            row()
                .key("brand")
                .px(space::S3)
                .pt(space::S1)
                .gap(space::S2)
                .center_y()
                .child(text(NAME).key("name").font(HEADING).color(p.t.text))
                .child(
                    text(VERSION)
                        .key("version")
                        .font(CAPTION)
                        .color(p.t.text_secondary),
                ),
        )
        .child(navigation)
        .child(spacer())
        .child(status(p, app))
        .child(
            col()
                .key("preview-note-box")
                .w_full()
                .px(space::S3)
                .child(caption(p, "preview-note", PREVIEW_NOTE).w_full()),
        )
        .child(button(p, "quit", "Quit ngine", Look::Plain, true, |app| {
            app.perform(Action::Quit);
        }))
}

struct Banner {
    key: &'static str,
    title: &'static str,
    detail: String,
    serious: bool,
}

fn problem_banner(problem: &Problem) -> Banner {
    let title = problem.summary();
    match problem {
        Problem::PermissionMissing(Permission::ScreenRecording) => Banner {
            key: "problem-permission-screen",
            title,
            detail: "Allow it in the system's privacy settings, then start again.".into(),
            serious: true,
        },
        Problem::PermissionMissing(Permission::Microphone) => Banner {
            key: "problem-permission-microphone",
            title,
            detail: "Allow it in the system's privacy settings, or switch the microphone off in Settings.".into(),
            serious: true,
        },
        Problem::CaptureRestarted => Banner {
            key: "problem-capture-restarted",
            title,
            detail: "Everything recorded before that is kept.".into(),
            serious: false,
        },
        Problem::CaptureLost => Banner {
            key: "problem-capture-lost",
            title,
            detail: "What was recorded up to then is saved.".into(),
            serious: true,
        },
        Problem::DiskFull { folder } => Banner {
            key: "problem-disk-full",
            title,
            detail: format!(
                "What was recorded up to then is saved. Free some space for {}.",
                folder.display()
            ),
            serious: true,
        },
    }
}

fn banner(p: &Paint, banner: Banner, dismiss: impl Fn(&mut Ngine) + 'static) -> El {
    let colour = if banner.serious { p.danger } else { p.warning };
    row()
        .key(banner.key)
        .role("alert")
        .label(banner.title)
        .px(space::S3)
        .py(space::S2)
        .gap(space::S3)
        .center_y()
        .radius(radius::MD)
        .fill(colour.alpha(0.12))
        .border(p.hair, colour.alpha(0.45))
        .child(
            el().key(format!("{}-dot", banner.key))
                .size(DOT, DOT)
                .radius(DOT / 2.0)
                .fill(colour)
                .a11y_hidden(),
        )
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
                        .color(p.t.text),
                )
                .child(caption(p, &format!("{}-detail", banner.key), banner.detail)),
        )
        .child(button(
            p,
            &format!("{}-dismiss", banner.key),
            "Dismiss",
            Look::Plain,
            true,
            dismiss,
        ))
}

fn banners(p: &Paint, app: &Ngine) -> Option<El> {
    if app.problems.is_empty() && !app.settings_were_reset {
        return None;
    }
    let mut stack = col().key("banners").gap(space::S2);
    if app.settings_were_reset {
        let reset = Banner {
            key: "banner-settings-reset",
            title: "The settings file was damaged",
            detail: "ngine set it aside and went back to the default settings.".into(),
            serious: false,
        };
        stack = stack.child(banner(p, reset, |app| app.settings_were_reset = false));
    }
    for (index, problem) in app.problems.iter().enumerate() {
        stack = stack.child(banner(p, problem_banner(problem), move |app| {
            app.dismiss_problem(index);
        }));
    }
    Some(stack)
}

fn notice(p: &Paint, app: &Ngine) -> Option<El> {
    let (words, colour) = match (&app.notice, &app.confirmation) {
        (Some(notice), _) if notice.is_error => (notice.text.clone(), p.danger),
        (_, Some(confirmation)) => (confirmation.text.clone(), p.good),
        (Some(notice), None) => (notice.text.clone(), p.t.text_secondary),
        (None, None) => return None,
    };
    Some(
        text(words)
            .key("notice")
            .role("status")
            .font(type_::LABEL)
            .color(colour),
    )
}

fn placeholder(p: &Paint, key: &str, title: &str, words: &str) -> El {
    col()
        .key(key)
        .w_full()
        .h(PLACEHOLDER_HEIGHT)
        .center()
        .gap(space::S2)
        .child(
            text(title)
                .key(format!("{key}-title"))
                .font(HEADING)
                .color(p.t.text),
        )
        .child(caption(p, &format!("{key}-words"), words))
        .child(caption(p, &format!("{key}-state"), "Not built yet"))
}

pub fn view(app: &Ngine, cx: &mut Cx<Ngine>) -> El {
    let p = &Paint::dark(cx.env().scale);
    let screen = match app.screen {
        Screen::Record => record::view(p, app),
        Screen::Settings => settings::view(p, app),
        Screen::Library => placeholder(
            p,
            "library",
            "Library",
            "Your recordings and clips will be listed here.",
        ),
        Screen::Editor => placeholder(
            p,
            "editor",
            "Clip editor",
            "Preview a recording, trim it and export a clip here.",
        ),
    };
    let mut content = col()
        .key("content")
        .w_full()
        .padding(CONTENT_PADDING)
        .gap(SECTION_GAP);
    if let Some(banners) = banners(p, app) {
        content = content.child(banners);
    }
    content = content.child(screen);
    if let Some(notice) = notice(p, app) {
        content = content.child(notice);
    }
    row()
        .key("window")
        .role("window")
        .w_full()
        .h_full()
        .fill(p.t.bg)
        .child(sidebar(p, app, cx.titlebar_inset() > 0.0))
        .child(
            col()
                .key("main")
                .grow(1.0)
                .h_full()
                .min_w(0.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(content),
        )
}

#[cfg(test)]
mod tests;
