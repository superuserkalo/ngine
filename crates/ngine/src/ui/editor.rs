use std::path::Path;
use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{
    Align, Color, Cursor, Drag, El, Phase, Side, SurfaceFit, col, el, icon, row, spacer, text,
};

use super::widgets::{
    Button, Choice, Field, Floating, HEADING, Look, NUMBERS, Paint, Segments, Stops, caption,
    empty_state, field, float, keycaps, lone_switch, section, segmented, slider, surface,
};
use super::{FirstRow, Ui, icons, page, sheet};
use crate::app::{Ngine, Popup, Screen, toggle_watching};
use crate::editor::{Editor, Exporting, FULL_VOLUME, Grip, LOUDEST_PERCENT, PERCENT_STEP, Source};
use crate::format;
use crate::platform;
use ngine_recorder::Cut;

pub const TIMELINE_WIDTH: f32 = 564.0;
const PANEL_WIDTH: f32 = 264.0;
const NARROW_PANEL_WIDTH: f32 = 232.0;
const NARROW_PANEL_BELOW: f32 = 720.0;
const ROOMY_TRANSPORT: f32 = 600.0;
const STEPS_FROM: f32 = 440.0;
const SIDE_BY_SIDE_FROM: f32 = 560.0;
const TIMELINE_HEIGHT: f32 = 40.0;
const BAR: f32 = 6.0;
const HANDLE: (f32, f32) = (6.0, 28.0);
const PLAYHEAD: (f32, f32) = (2.0, 36.0);
const MARKER: (f32, f32) = (2.0, 10.0);
const GRAB: f32 = 10.0;
const PROGRESS_HEIGHT: f32 = 6.0;
const CUT_INSET: f32 = space::S3 + 2.0;
const WIDEST_SEGMENT: f32 = 120.0;
const PREVIEW_VOLUME_WIDTH: f32 = 72.0;
const LEVEL_WIDTH: f32 = 140.0;
const EXPAND: f32 = 32.0;
const OVERLAY_WIDTH: f32 = 960.0;
const OVERLAY_ROOM: f32 = 140.0;
const HEADER_NEEDS: f32 = 400.0;
const HEADER_ROW: f32 = 42.0;
const BELOW_VIDEO: f32 = 148.0;
const SMALLEST_VIDEO: f32 = 160.0;
const SCRUB: f32 = 0.0;
const MOVE_IN: f32 = 1.0;
const MOVE_OUT: f32 = 2.0;

pub const SHORTCUTS: [(&[&str], &str); 9] = [
    (&["Space"], "Play or pause"),
    (&["←", "→"], "One frame back or forward"),
    (&["⇧", "←"], "One second back"),
    (&["⇧", "→"], "One second forward"),
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
                .color(p.t.text),
        )
}

fn sentence(words: &str) -> String {
    if words.ends_with(['.', '!', '?']) {
        words.to_owned()
    } else {
        format!("{words}.")
    }
}

fn with_editor(change: impl Fn(&mut Editor) + 'static) -> impl Fn(&mut Ngine) + 'static {
    move |app: &mut Ngine| {
        if let Some(editor) = &mut app.editor {
            change(editor);
        }
    }
}

fn transport(p: &Paint, editor: &Editor, width: f32) -> El {
    let narrow = width < ROOMY_TRANSPORT;
    let (play, glyph) = if editor.playing {
        ("Pause", &icons::PAUSE)
    } else {
        ("Play", &icons::PLAY)
    };
    let key = |cap: &str| (!narrow).then(|| vec![cap.to_owned()]);
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
        );
    if width >= STEPS_FROM {
        bar = bar
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
            );
    }
    bar = bar
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
        .px(space::S3 + 2.0)
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

fn cut_at(p: &Paint, editor: &Editor, width: f32) -> El {
    let segment_width = (((width - 2.0 * CUT_INSET) / 2.0).floor() - 2.0).min(WIDEST_SEGMENT);
    let cuts = [Cut::Fast, Cut::Exact];
    let control = segmented(
        p,
        Segments {
            key: "trim-cut",
            label: "Cut at",
            choices: vec![
                Choice::new("keyframe", "Keyframe"),
                Choice::new("exact", "Exact frame"),
            ],
            selected: cuts.iter().position(|cut| *cut == editor.cut),
            segment_width,
        },
        move |app, index| {
            if let (Some(editor), Some(cut)) = (&mut app.editor, cuts.get(index)) {
                editor.cut = *cut;
            }
        },
    )
    .help("Keyframe is instant. Exact frame re-encodes the ends of the clip.");
    col()
        .key("trim-cut-row")
        .px(CUT_INSET)
        .pt(space::S2)
        .gap(space::S2)
        .child(
            text("Cut at")
                .key("trim-cut-label")
                .font(type_::BODY)
                .color(p.t.text),
        )
        .child(control)
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
        .look(Look::Primary)
        .build(p, Ngine::export_clip)
        .w_full()
}

fn exported(p: &Paint, body: El, file: &Path, length: Duration) -> El {
    let shown = file.to_owned();
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    body.child(
        text(format!("Saved · {}", format::length(length)))
            .key("export-done")
            .help(name)
            .role("status")
            .font(type_::LABEL)
            .color(p.good)
            .wrap(),
    )
    .child(
        row()
            .key("export-after")
            .gap(space::S2)
            .flex_wrap()
            .child(
                Button::new(
                    "export-show",
                    &format!("Show in {}", platform::FILE_MANAGER),
                )
                .icon(&icons::FOLDER_OPEN)
                .look(Look::Primary)
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

fn export(p: &Paint, app: &Ngine, editor: &Editor, width: f32) -> El {
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
    if matches!(
        editor.exporting,
        Exporting::Idle | Exporting::Running { .. }
    ) {
        body = body.child(caption(
            p,
            "export-words",
            format!("Saved to {}", app.settings.output_folder.display()),
        ));
    }
    body = match &editor.exporting {
        Exporting::Idle => body.child(
            Button::new("export", "Export clip")
                .icon(&icons::SCISSORS)
                .look(Look::Primary)
                .build(p, Ngine::export_clip)
                .w_full(),
        ),
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
        Exporting::Done { file, length } => exported(p, body, file, *length),
        Exporting::Failed(error) => body
            .child(
                text(format!(
                    "The clip could not be exported. {}",
                    sentence(error)
                ))
                .key("export-failed")
                .role("alert")
                .font(type_::LABEL)
                .color(p.danger)
                .wrap(),
            )
            .child(export_button(p, "Try again")),
    };
    let mut group = col()
        .key("export-section")
        .padding(space::S1)
        .fill(p.t.card)
        .radius(radius::LG)
        .border(p.hair, p.t.card_border)
        .child(cut_at(p, editor, width - 2.0 * space::S1));
    group = group.child(body);
    group
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
                .build(p, Ngine::close_editor)
                .ml(-space::S3),
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
            text(format!("{} × {}", editor.info.width, editor.info.height))
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

fn fitted(aspect: f32, width: f32, height: f32) -> (f32, f32) {
    if width / height > aspect {
        (height * aspect, height)
    } else {
        (width, width / aspect)
    }
}

fn preview(p: &Paint, editor: &Editor, width: f32, height: f32) -> El {
    let (shown_width, shown_height) = fitted(aspect(editor), width, height);
    let mut picture = el()
        .key("preview")
        .w(shown_width)
        .h(shown_height)
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
        picture = picture.child(expand_button(false).absolute(
            shown_width - EXPAND - space::S3,
            shown_height - EXPAND - space::S3,
        ));
    }
    if let Some(failure) = &editor.failure {
        picture = picture.child(
            text(format!("The video cannot be shown: {failure}"))
                .key("preview-failure")
                .role("alert")
                .font(type_::LABEL)
                .color(p.danger),
        );
    }
    el().key("preview-area")
        .w(width)
        .h(height)
        .center()
        .child(picture)
}

fn aspect(editor: &Editor) -> f32 {
    let side = |pixels: u32| f32::from(u16::try_from(pixels).unwrap_or(u16::MAX));
    let (width, height) = (side(editor.info.width), side(editor.info.height));
    if width > 0.0 && height > 0.0 {
        width / height
    } else {
        16.0 / 9.0
    }
}

struct Layout {
    side_by_side: bool,
    panel: f32,
    video: (f32, f32),
}

impl Layout {
    fn fit(ui: &mut Ui<'_>, aspect: f32) -> Self {
        let column = ui.content_width();
        let side_by_side = column >= SIDE_BY_SIDE_FROM && ui.window.0 >= ui.window.1;
        let panel = if column < NARROW_PANEL_BELOW {
            NARROW_PANEL_WIDTH
        } else {
            PANEL_WIDTH
        };
        let beside = if side_by_side { panel + space::S6 } else { 0.0 };
        let widest = (ui.room_across() - beside).max(1.0);
        let mut layout = Self::sized(
            ui,
            aspect,
            side_by_side,
            panel,
            widest,
            FirstRow::BesideControls,
        );
        if !ui.fits_top_row(HEADER_NEEDS) {
            layout = Self::sized(ui, aspect, side_by_side, panel, widest, FirstRow::InContent);
        }
        layout
    }

    fn sized(
        ui: &mut Ui<'_>,
        aspect: f32,
        side_by_side: bool,
        panel: f32,
        widest: f32,
        first_row: FirstRow,
    ) -> Self {
        let header = match first_row {
            FirstRow::BesideControls => 0.0,
            FirstRow::InContent => HEADER_ROW,
        };
        let tallest =
            (ui.window.1 - ui.content_top(first_row) - header - BELOW_VIDEO).max(SMALLEST_VIDEO);
        let beside = if side_by_side { panel + space::S6 } else { 0.0 };
        let width = widest.min(tallest * aspect);
        let video = (width, width / aspect);
        ui.widen_to(video.0 + beside);
        Self {
            side_by_side,
            panel,
            video,
        }
    }
}

pub fn view(ui: &mut Ui<'_>, timeline_width: f32) -> El {
    let p = ui.p;
    let app = ui.app;
    let Some(editor) = &app.editor else {
        return page("editor", sheet(ui, "editor").child(empty(&p)));
    };
    let keys_open = app.popup == Some(Popup::EditorKeys);
    let layout = Layout::fit(ui, aspect(editor));
    let (side_by_side, panel_width) = (layout.side_by_side, layout.panel);
    let (video_width, video_height) = layout.video;
    let mut stage = col()
        .key("stage")
        .w(video_width)
        .gap(space::S3)
        .child(preview(&p, editor, video_width, video_height))
        .child(transport(&p, editor, video_width))
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
    let mut panel = col().key("editor-panel").gap(space::S6).child(export(
        &p,
        app,
        editor,
        if side_by_side {
            panel_width
        } else {
            video_width
        },
    ));
    if let Some(sound) = sound(&p, editor) {
        panel = panel.child(sound);
    }
    let body = if side_by_side {
        row()
            .key("editor-body")
            .gap(space::S6)
            .items_start()
            .child(stage)
            .child(panel.w(panel_width))
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
        let (shown_width, shown_height) = fitted(aspect(editor), width, height);
        let margin = if shown_height < 2.0 * OVERLAY_ROOM {
            space::S3
        } else {
            space::S6
        };
        let overlay_width = (shown_width - 2.0 * margin).min(OVERLAY_WIDTH);
        let foot = (height - shown_height) / 2.0 + margin;
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
                        transport(&p, editor, overlay_width - EXPAND - space::S2)
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
                .pin(Some(0.0), None, Some(0.0), Some(foot))
                .justify_center()
                .child(controls),
        );
    }
    Some(stage)
}
