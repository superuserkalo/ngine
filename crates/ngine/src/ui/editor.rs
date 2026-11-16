use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{
    Align, Color, Cursor, Drag, El, Phase, Side, SurfaceFit, col, el, icon, row, spacer, text,
};

use super::widgets::{
    Button, Field, Floating, HEADING, Look, NUMBERS, Paint, Stops, caption, empty_state, field,
    float, keycaps, lone_switch, section, slider, surface, toggle,
};
use super::{Ui, icons, page, sheet};
use crate::app::{Ngine, Popup, Screen, toggle_watching};
use crate::editor::{Editor, Exporting, FULL_VOLUME, Grip, LOUDEST_PERCENT, PERCENT_STEP, Source};
use crate::format;
use crate::platform;
use ngine_recorder::Cut;

pub const TIMELINE_WIDTH: f32 = 564.0;
const PANEL_WIDTH: f32 = 264.0;
const SIDE_BY_SIDE_FROM: f32 = 600.0;
const TIMELINE_HEIGHT: f32 = 40.0;
const BAR: f32 = 6.0;
const HANDLE: (f32, f32) = (6.0, 28.0);
const PLAYHEAD: (f32, f32) = (2.0, 36.0);
const MARKER: (f32, f32) = (2.0, 10.0);
const GRAB: f32 = 10.0;
const PROGRESS_HEIGHT: f32 = 6.0;
const PREVIEW_VOLUME_WIDTH: f32 = 72.0;
const LEVEL_WIDTH: f32 = 140.0;
const EXPAND: f32 = 32.0;
const OVERLAY_WIDTH: f32 = 960.0;
const HEADER_NEEDS: f32 = 400.0;
const SCRUB: f32 = 0.0;
const MOVE_IN: f32 = 1.0;
const MOVE_OUT: f32 = 2.0;

pub const SHORTCUTS: [(&[&str], &str); 8] = [
    (&["Space"], "Play or pause"),
    (&["←", "→"], "One frame back or forward"),
    (&["⇧", "←"], "One second back, with → forward"),
    (&["I"], "Mark in: where the clip starts"),
    (&["O"], "Mark out: where the clip ends"),
    (&["F"], "Full screen, and back"),
    (&["Esc"], "Back, or leave full screen"),
    (&["?"], "Show these keys"),
];

fn fraction(editor: &Editor, at: Duration) -> f32 {
    let length = editor.length().as_secs_f32();
    if length <= 0.0 {
        0.0
    } else {
        (at.as_secs_f32() / length).clamp(0.0, 1.0)
    }
}

fn grabbed(editor: &Editor, x: f32, width: f32) -> f32 {
    let to_in = (fraction(editor, editor.start) * width - x).abs();
    let to_out = (fraction(editor, editor.end) * width - x).abs();
    if to_in.min(to_out) > GRAB {
        SCRUB
    } else if to_in < to_out {
        MOVE_IN
    } else {
        MOVE_OUT
    }
}

fn drag_timeline(editor: &mut Editor, drag: &mut Drag) {
    let width = drag.w.max(1.0);
    if drag.phase == Phase::Down {
        drag.data = grabbed(editor, drag.x, width);
    }
    if drag.phase == Phase::Up {
        return;
    }
    let at = editor.length().mul_f32((drag.x / width).clamp(0.0, 1.0));
    if drag.data < f32::midpoint(SCRUB, MOVE_IN) {
        editor.pause();
        editor.seek(at);
    } else if drag.data < f32::midpoint(MOVE_IN, MOVE_OUT) {
        editor.set_start(at);
    } else {
        editor.set_end(at);
    }
}

fn mark(key: String, size: (f32, f32), centre: f32, colour: Color) -> El {
    el().key(key)
        .size(size.0, size.1)
        .absolute(centre - size.0 / 2.0, (TIMELINE_HEIGHT - size.1) / 2.0)
        .radius(size.0.min(3.0) / 2.0)
        .fill(colour)
        .a11y_hidden()
}

fn timeline(p: &Paint, editor: &Editor, width: f32) -> El {
    let at = |time: Duration| fraction(editor, time) * width;
    let (from, to) = (at(editor.start), at(editor.end));
    let bar_top = (TIMELINE_HEIGHT - BAR) / 2.0;
    let mut line = el()
        .key("timeline")
        .role("slider")
        .label("Timeline")
        .range_value(
            editor.position.as_secs_f64(),
            0.0,
            editor.length().as_secs_f64(),
        )
        .a11y_value(format::position(editor.position))
        .help("Drag to move through the video. Drag the ends of the clip to trim it.")
        .focusable(true)
        .w_full()
        .h(TIMELINE_HEIGHT)
        .cursor(match editor.grip {
            Grip::ClipEnd => Cursor::ResizeColumn,
            Grip::Playhead => Cursor::Pointer,
        })
        .on_pointer_move_local(move |app: &mut Ngine, pointer, _| {
            if let Some(editor) = &mut app.editor {
                editor.grip = if grabbed(editor, pointer.x, width.max(1.0)) > SCRUB {
                    Grip::ClipEnd
                } else {
                    Grip::Playhead
                };
            }
        })
        .on_step(|app: &mut Ngine, steps, _| {
            if let Some(editor) = &mut app.editor {
                editor.step(steps);
            }
        })
        .on_drag(|app: &mut Ngine, drag: &mut Drag, tx| {
            if let Some(editor) = &mut app.editor {
                drag_timeline(editor, drag);
                tx.instant();
            }
        })
        .child(
            el().key("timeline-track")
                .size(width, BAR)
                .absolute(0.0, bar_top)
                .radius(BAR / 2.0)
                .fill(p.t.control_off)
                .a11y_hidden(),
        )
        .child(
            el().key("timeline-clip")
                .size((to - from).max(1.0), BAR)
                .absolute(from, bar_top)
                .fill(p.accent.alpha(0.55))
                .a11y_hidden(),
        );
    for (index, marker) in editor.markers.iter().enumerate() {
        line = line.child(mark(
            format!("marker-{index}"),
            MARKER,
            at(*marker),
            p.warning,
        ));
    }
    let inside = |centre: f32| centre.clamp(HANDLE.0 / 2.0, (width - HANDLE.0 / 2.0).max(0.0));
    line.child(mark("in-point".into(), HANDLE, inside(from), p.accent))
        .child(mark("out-point".into(), HANDLE, inside(to), p.accent))
        .child(mark(
            "playhead".into(),
            PLAYHEAD,
            at(editor.position),
            p.t.text,
        ))
}

fn range(p: &Paint, editor: &Editor) -> El {
    row()
        .key("clip-range")
        .gap(space::S2)
        .center_y()
        .child(
            text(format!(
                "{} – {}",
                format::position(editor.start),
                format::position(editor.end)
            ))
            .key("clip-points")
            .font(NUMBERS)
            .color(p.t.text_secondary),
        )
        .child(spacer())
        .child(
            text(format!("Clip {}", format::length(editor.clip_length())))
                .key("clip-length")
                .font(NUMBERS)
                .color(p.accent),
        )
}

fn with_editor(change: impl Fn(&mut Editor) + 'static) -> impl Fn(&mut Ngine) + 'static {
    move |app: &mut Ngine| {
        if let Some(editor) = &mut app.editor {
            change(editor);
        }
    }
}

fn transport(p: &Paint, editor: &Editor, narrow: bool) -> El {
    let (play, glyph) = if editor.playing {
        ("Pause", &icons::PAUSE)
    } else {
        ("Play", &icons::PLAY)
    };
    let key = |cap: &str| Some(vec![cap.to_owned()]);
    let mut bar = row()
        .key("transport")
        .gap(space::S1 + 2.0)
        .flex_wrap()
        .center_y()
        .child(
            Button::new("play", play)
                .only_icon(glyph)
                .look(Look::Plain)
                .help(format!("{play} (Space)"))
                .build(p, with_editor(Editor::toggle_play)),
        )
        .child(
            Button::new("step-back", "One frame back")
                .only_icon(&icons::STEP_BACK)
                .look(Look::Quiet)
                .build(p, with_editor(|editor| editor.step(-1))),
        )
        .child(
            Button::new("step-forward", "One frame forward")
                .only_icon(&icons::STEP_FORWARD)
                .look(Look::Quiet)
                .build(p, with_editor(|editor| editor.step(1))),
        )
        .child(
            text(format!(
                "{} / {}",
                format::position(editor.position),
                format::position(editor.length())
            ))
            .key("position")
            .font(NUMBERS)
            .color(p.t.text)
            .ml(space::S1),
        )
        .child(spacer())
        .child(
            Button::new("set-in", "Mark in")
                .keys(key("I"))
                .help("Start the clip here (I)")
                .build(p, with_editor(|editor| editor.set_start(editor.position))),
        )
        .child(
            Button::new("set-out", "Mark out")
                .keys(key("O"))
                .help("End the clip here (O)")
                .build(p, with_editor(|editor| editor.set_end(editor.position))),
        );
    if !narrow {
        let stops = Stops {
            key: "preview-volume",
            label: "Preview volume",
            index: u16::from(editor.volume() / 10),
            count: u16::from(FULL_VOLUME / 10) + 1,
            value: format!("{} %", editor.volume()),
            width: PREVIEW_VOLUME_WIDTH,
        };
        bar = bar
            .child(
                el().key("preview-volume-icon")
                    .ml(space::S2)
                    .child(super::widgets::glyph(
                        p,
                        "preview-volume-glyph",
                        &icons::VOLUME_2,
                        15.0,
                    )),
            )
            .child(slider(p, stops, |app, stop| {
                if let Some(editor) = &mut app.editor {
                    editor.set_volume(u8::try_from(stop * 10).unwrap_or(FULL_VOLUME));
                }
            }));
    }
    bar
}

fn source_row(p: &Paint, editor: &Editor, source: Source) -> El {
    let (key, label) = match source {
        Source::System => ("clip-system", "System sound"),
        Source::Microphone => ("clip-microphone", "Microphone"),
    };
    let level = editor.level(source);
    let stops = Stops {
        key: &format!("{key}-volume"),
        label: &format!("{label} level in the clip"),
        index: level.percent / PERCENT_STEP,
        count: LOUDEST_PERCENT / PERCENT_STEP + 1,
        value: format!("{} %", level.percent),
        width: LEVEL_WIDTH,
    };
    let volume = slider(p, stops, move |app, stop| {
        if let Some(editor) = &mut app.editor {
            editor.set_level(source, |level| level.percent = stop * PERCENT_STEP);
        }
    })
    .opacity(if level.on { 1.0 } else { 0.4 });
    col()
        .key(key)
        .px(space::S3)
        .py(space::S2)
        .gap(space::S2)
        .child(
            row()
                .key(format!("{key}-head"))
                .center_y()
                .child(
                    text(label)
                        .key(format!("{key}-label"))
                        .font(type_::BODY)
                        .color(p.t.text),
                )
                .child(spacer())
                .child(lone_switch(
                    p,
                    &format!("{key}-on"),
                    label,
                    level.on,
                    move |app| {
                        if let Some(editor) = &mut app.editor {
                            editor.set_level(source, |level| level.on = !level.on);
                        }
                    },
                )),
        )
        .child(volume)
}

fn sound(p: &Paint, editor: &Editor) -> Option<El> {
    editor.info.has_separate_sound().then(|| {
        section(
            p,
            "clip-sound",
            "Sound",
            vec![
                source_row(p, editor, Source::System),
                source_row(p, editor, Source::Microphone),
            ],
        )
    })
}

fn precise_trim(p: &Paint, editor: &Editor) -> El {
    let precise = editor.cut == Cut::Exact;
    let hint = if precise {
        "Re-encodes the ends to cut on the frame"
    } else {
        "Cuts on keyframes and re-encodes nothing"
    };
    toggle(
        p,
        "precise-trim",
        "Precise trim",
        Some(hint.into()),
        precise,
        with_editor(|editor| {
            editor.cut = if editor.cut == Cut::Exact {
                Cut::Fast
            } else {
                Cut::Exact
            };
        }),
    )
}

fn progress(p: &Paint, done: f32) -> El {
    el().key("export-progress")
        .role("progressbar")
        .label("Export")
        .range_value(f64::from(done), 0.0, 1.0)
        .w_full()
        .h(PROGRESS_HEIGHT)
        .radius(PROGRESS_HEIGHT / 2.0)
        .fill(p.t.control_off)
        .child(
            el().key("export-progress-fill")
                .w_frac(done.clamp(0.02, 1.0))
                .h(PROGRESS_HEIGHT)
                .radius(PROGRESS_HEIGHT / 2.0)
                .fill(p.accent)
                .a11y_hidden(),
        )
}

fn export_button(p: &Paint, label: &str) -> El {
    Button::new("export", label)
        .icon(&icons::SCISSORS)
        .look(Look::Primary)
        .build(p, Ngine::export_clip)
        .w_full()
}

fn export(p: &Paint, app: &Ngine, editor: &Editor) -> El {
    let name = field(
        p,
        &Field {
            key: "clip-name",
            label: "Clip name",
            edit: &app.clip_name,
            placeholder: "Name (optional)",
            icon: None,
        },
        |app| &mut app.clip_name,
        |_| {},
    );
    let mut body = col()
        .key("export-body")
        .px(space::S3 + 2.0)
        .pt(space::S2)
        .pb(space::S3)
        .gap(space::S2)
        .child(name);
    if !matches!(editor.exporting, Exporting::Done { .. }) {
        body = body.child(caption(
            p,
            "export-words",
            format!(
                "{} MP4, saved to {}",
                format::length(editor.clip_length()),
                app.settings.output_folder.display()
            ),
        ));
    }
    body = match &editor.exporting {
        Exporting::Idle => body.child(export_button(p, "Export clip")),
        Exporting::Running { done, .. } => body.child(progress(p, *done)).child(
            row()
                .key("export-running")
                .center_y()
                .child(
                    text(format!("Exporting… {:.0} %", done * 100.0))
                        .key("export-percent")
                        .font(NUMBERS)
                        .color(p.t.text),
                )
                .child(spacer())
                .child(
                    Button::new("export-cancel", "Cancel")
                        .build(p, with_editor(Editor::cancel_export)),
                ),
        ),
        Exporting::Done { file, length } => {
            let shown = file.clone();
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            body.child(
                text(format!("Saved {name} · {}", format::length(*length)))
                    .key("export-done")
                    .role("status")
                    .font(type_::LABEL)
                    .color(p.good)
                    .wrap(),
            )
            .child(
                row()
                    .key("export-after")
                    .gap(space::S2)
                    .child(
                        Button::new(
                            "export-show",
                            &format!("Show in {}", platform::FILE_MANAGER),
                        )
                        .icon(&icons::FOLDER_OPEN)
                        .build(p, move |app| app.reveal(&shown))
                        .grow(1.0),
                    )
                    .child(
                        Button::new("export", "Export again")
                            .build(p, Ngine::export_clip)
                            .grow(1.0),
                    ),
            )
        }
        Exporting::Failed(error) => body
            .child(
                text(format!("The clip could not be exported: {error}"))
                    .key("export-failed")
                    .role("alert")
                    .font(type_::LABEL)
                    .color(p.danger)
                    .wrap(),
            )
            .child(export_button(p, "Try again")),
    };
    section(
        p,
        "export-section",
        "Export",
        vec![precise_trim(p, editor), body],
    )
}

fn header(p: &Paint, editor: &Editor, keys_open: bool) -> El {
    row()
        .key("editor-header")
        .gap(space::S2)
        .center_y()
        .child(
            Button::new("editor-back", "Library")
                .icon(&icons::ARROW_LEFT)
                .look(Look::Quiet)
                .help("Back to the library (Esc)")
                .build(p, Ngine::close_editor),
        )
        .child(
            text(editor.name.clone())
                .key("editor-name")
                .role("heading")
                .level(1)
                .font(HEADING)
                .color(p.t.text)
                .truncate()
                .min_w(0.0)
                .shrink(),
        )
        .child(
            text(format!(
                "{} · {} × {}",
                format::length(editor.length()),
                editor.info.width,
                editor.info.height
            ))
            .key("editor-details")
            .font(NUMBERS)
            .color(p.faint),
        )
        .child(spacer())
        .child(
            Button::new("editor-keys-button", "Keyboard shortcuts")
                .only_icon(&icons::CIRCLE_QUESTION_MARK)
                .look(Look::Quiet)
                .opens(keys_open)
                .build(p, move |app| {
                    if keys_open {
                        app.close_popup();
                    } else {
                        app.open_popup(Popup::EditorKeys);
                    }
                }),
        )
}

fn shortcuts(ui: &mut Ui<'_>) {
    let p = ui.p;
    let mut list = surface(&p, "editor-keys", "dialog", "Keyboard shortcuts")
        .padding(space::S3)
        .gap(space::S2)
        .w(300.0);
    for (index, (caps, words)) in SHORTCUTS.iter().enumerate() {
        let caps: Vec<String> = caps.iter().map(|cap| (*cap).to_owned()).collect();
        list = list.child(
            row()
                .key(format!("shortcut-{index}"))
                .gap(space::S3)
                .center_y()
                .child(
                    el().key(format!("shortcut-{index}-keys"))
                        .w(72.0)
                        .child(keycaps(&p, &format!("shortcut-{index}-caps"), &caps)),
                )
                .child(caption(&p, &format!("shortcut-{index}-words"), *words)),
        );
    }
    let floating = Floating {
        key: "editor-keys",
        anchor: "editor-keys-button",
        side: Side::Bottom,
        align: Align::End,
    };
    float(&mut ui.layer, &floating, list);
}

fn empty(p: &Paint) -> El {
    empty_state(
        p,
        "editor-empty",
        &icons::SCISSORS,
        "No recording open",
        "Open a recording from the library to trim it and export a clip.".into(),
        Some(
            Button::new("editor-to-library", "Go to the library")
                .build(p, |app| app.show(Screen::Library)),
        ),
    )
}

fn expand_button(watching: bool) -> El {
    let (label, glyph) = if watching {
        ("Exit full screen", &icons::MINIMIZE_2)
    } else {
        ("Full screen", &icons::MAXIMIZE_2)
    };
    el().key("full-screen")
        .role("button")
        .label(label)
        .help(format!("{label} (F)"))
        .size(EXPAND, EXPAND)
        .center()
        .radius(radius::MD)
        .fill(Color::BLACK.alpha(0.55))
        .hover(Color::BLACK.alpha(0.75))
        .press(Color::BLACK.alpha(0.9))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(toggle_watching)
        .child(icon(glyph, 16.0).color(Color::WHITE).a11y_hidden())
}

fn preview(p: &Paint, editor: &Editor, width: f32, height: f32) -> El {
    let mut frame = el()
        .key("preview")
        .w(width)
        .h(height)
        .radius(radius::LG)
        .fill(Color::BLACK)
        .center()
        .pixels_fit(&editor.picture, SurfaceFit::Fit)
        .on_hover_change(|app: &mut Ngine, inside, _| {
            if let Some(editor) = &mut app.editor {
                editor.hovered = inside;
            }
        });
    if editor.hovered {
        frame = frame.child(
            expand_button(false).absolute(width - EXPAND - space::S3, height - EXPAND - space::S3),
        );
    }
    if let Some(failure) = &editor.failure {
        frame = frame.child(
            text(format!("The video cannot be shown: {failure}"))
                .key("preview-failure")
                .role("alert")
                .font(type_::LABEL)
                .color(p.danger),
        );
    }
    frame
}

pub fn view(ui: &mut Ui<'_>, timeline_width: f32) -> El {
    let p = ui.p;
    let app = ui.app;
    let Some(editor) = &app.editor else {
        return page("editor", sheet(ui, "editor").child(empty(&p)));
    };
    let keys_open = app.popup == Some(Popup::EditorKeys);
    let width = ui.content_width();
    let side_by_side = width >= SIDE_BY_SIDE_FROM && ui.window.0 >= ui.window.1;
    let video_width = if side_by_side {
        width - PANEL_WIDTH - space::S6
    } else {
        width
    };
    let tallest = (ui.window.1 - 260.0).max(160.0);
    let video_height = (video_width * 9.0 / 16.0).min(tallest);
    let mut stage = col()
        .key("stage")
        .w(video_width)
        .gap(space::S3)
        .child(preview(&p, editor, video_width, video_height))
        .child(transport(&p, editor, video_width < 600.0))
        .child(
            col()
                .key("timeline-block")
                .gap(space::S1)
                .child(range(&p, editor))
                .child(timeline(&p, editor, timeline_width)),
        );
    if editor.plays_silently() {
        stage = stage.child(caption(
            &p,
            "preview-silent",
            "No sound output could be opened, so the preview plays without sound. The clip keeps its sound.",
        ));
    }
    let mut panel = col().key("editor-panel").gap(space::S6);
    if let Some(sound) = sound(&p, editor) {
        panel = panel.child(sound);
    }
    panel = panel.child(export(&p, app, editor));
    let body = if side_by_side {
        row()
            .key("editor-body")
            .gap(space::S6)
            .items_start()
            .child(stage)
            .child(panel.w(PANEL_WIDTH))
    } else {
        col()
            .key("editor-body")
            .gap(space::S6 - 4.0)
            .child(stage)
            .child(panel.w_full())
    };
    if keys_open {
        shortcuts(ui);
    }
    let first = ui.top_row(header(&p, editor, keys_open), HEADER_NEEDS);
    let mut content = sheet(ui, "editor").gap(space::S3);
    if let Some(header) = first {
        content = content.child(header);
    }
    page("editor", content.child(body))
}

/// The video filling the window, with the transport and the timeline over
/// its foot while the pointer moves or the video is paused.
pub fn watched(ui: &mut Ui<'_>, timeline_width: f32) -> Option<El> {
    let p = ui.p;
    let app = ui.app;
    let editor = app.editor.as_ref()?;
    let (width, height) = ui.window;
    let mut stage = el()
        .key("watching")
        .w(width)
        .h(height)
        .fill(Color::BLACK)
        .pixels_fit(&editor.picture, SurfaceFit::Fit)
        .on_pointer_move(|app: &mut Ngine, _| app.pointer_moved(false));
    if editor.controls_shown(app.clock()) {
        let overlay_width = (width - 2.0 * space::S6).min(OVERLAY_WIDTH);
        let controls = col()
            .key("watching-controls")
            .w(overlay_width)
            .px(space::S3 + space::S1)
            .pt(space::S3)
            .pb(space::S2)
            .gap(space::S1)
            .radius(radius::LG)
            .fill(Color::BLACK.alpha(0.72))
            .border(p.hair, Color::WHITE.alpha(0.08))
            .on_pointer_move(|app: &mut Ngine, _| app.pointer_moved(true))
            .on_hover_change(|app: &mut Ngine, inside, _| app.pointer_moved(inside))
            .child(
                row()
                    .key("watching-top")
                    .gap(space::S2)
                    .center_y()
                    .child(
                        transport(&p, editor, overlay_width < 600.0)
                            .grow(1.0)
                            .min_w(0.0),
                    )
                    .child(expand_button(true)),
            )
            .child(
                col()
                    .key("timeline-block")
                    .gap(space::S1)
                    .child(range(&p, editor))
                    .child(timeline(&p, editor, timeline_width)),
            );
        stage = stage.child(
            row()
                .key("watching-foot")
                .pin(Some(0.0), None, Some(0.0), Some(space::S6))
                .justify_center()
                .child(controls),
        );
    }
    Some(stage)
}
