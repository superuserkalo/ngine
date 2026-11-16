use std::time::Duration;

use rune::tok::{radius, space, type_};
use rune::{Color, Drag, El, Phase, SurfaceFit, col, el, row, spacer, text};

use super::widgets::{
    Choice, HEADING, Look, Paint, Segments, Stops, button, caption, section, segmented, setting,
    slider, switch,
};
use crate::app::{Ngine, Screen};
use crate::editor::{Editor, Exporting, LOUDEST_PERCENT, PERCENT_STEP, Source};
use crate::format;
use crate::platform;
use ngine_recorder::Cut;

pub const TIMELINE_WIDTH: f32 = 564.0;
const EMPTY_HEIGHT: f32 = 420.0;
const PREVIEW_HEIGHT: f32 = 270.0;
const TIMELINE_HEIGHT: f32 = 44.0;
const BAR: f32 = 8.0;
const HANDLE: (f32, f32) = (10.0, 30.0);
const PLAYHEAD: (f32, f32) = (2.0, 40.0);
const MARKER: (f32, f32) = (3.0, 12.0);
const GRAB: f32 = 9.0;
const PROGRESS: (f32, f32) = (220.0, 6.0);
const CUT_WIDTH: f32 = 72.0;
const SCRUB: f32 = 0.0;
const MOVE_IN: f32 = 1.0;
const MOVE_OUT: f32 = 2.0;

fn fraction(editor: &Editor, at: Duration) -> f32 {
    let length = editor.length().as_secs_f32();
    if length <= 0.0 {
        0.0
    } else {
        (at.as_secs_f32() / length).clamp(0.0, 1.0)
    }
}

fn drag_timeline(editor: &mut Editor, drag: &mut Drag) {
    let width = drag.w.max(1.0);
    if drag.phase == Phase::Down {
        let to_in = (fraction(editor, editor.start) * width - drag.x).abs();
        let to_out = (fraction(editor, editor.end) * width - drag.x).abs();
        drag.data = if to_in.min(to_out) > GRAB {
            SCRUB
        } else if to_in < to_out {
            MOVE_IN
        } else {
            MOVE_OUT
        };
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
        .focusable(true)
        .w_full()
        .h(TIMELINE_HEIGHT)
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
                .absolute(0.0, (TIMELINE_HEIGHT - BAR) / 2.0)
                .radius(BAR / 2.0)
                .fill(p.t.control_off)
                .a11y_hidden(),
        )
        .child(
            el().key("timeline-clip")
                .size((to - from).max(1.0), BAR)
                .absolute(from, (TIMELINE_HEIGHT - BAR) / 2.0)
                .fill(p.accent.alpha(0.6))
                .a11y_hidden(),
        );
    for (index, marker) in editor.markers.iter().enumerate() {
        line = line.child(
            mark(format!("marker-{index}"), MARKER, at(*marker), p.warning)
                .absolute(at(*marker) - MARKER.0 / 2.0, 2.0),
        );
    }
    line.child(mark("in-point".into(), HANDLE, from, p.accent))
        .child(mark("out-point".into(), HANDLE, to, p.accent))
        .child(mark(
            "playhead".into(),
            PLAYHEAD,
            at(editor.position),
            p.t.text,
        ))
}

fn transport(p: &Paint, editor: &Editor) -> El {
    let play = if editor.playing { "Pause" } else { "Play" };
    row()
        .key("transport")
        .gap(space::S2)
        .center_y()
        .child(button(p, "play", play, Look::Primary, true, |app| {
            if let Some(editor) = &mut app.editor {
                editor.toggle_play();
            }
        }))
        .child(
            text(format!(
                "{} / {}",
                format::position(editor.position),
                format::position(editor.length())
            ))
            .key("position")
            .font(type_::LABEL.features("tnum"))
            .color(p.t.text)
            .ml(space::S2),
        )
        .child(spacer())
        .child(caption(
            p,
            "clip-length",
            format!(
                "Clip {} to {}  ·  {}",
                format::position(editor.start),
                format::position(editor.end),
                format::length(editor.clip_length())
            ),
        ))
        .child(button(p, "set-in", "Set in", Look::Plain, true, |app| {
            if let Some(editor) = &mut app.editor {
                editor.set_start(editor.position);
            }
        }))
        .child(button(p, "set-out", "Set out", Look::Plain, true, |app| {
            if let Some(editor) = &mut app.editor {
                editor.set_end(editor.position);
            }
        }))
}

fn source_row(p: &Paint, editor: &Editor, source: Source) -> El {
    let (key, label) = match source {
        Source::System => ("clip-system", "System sound"),
        Source::Microphone => ("clip-microphone", "Microphone"),
    };
    let level = editor.level(source);
    let stops = Stops {
        key: &format!("{key}-volume"),
        label: &format!("{label} volume"),
        index: level.percent / PERCENT_STEP,
        count: LOUDEST_PERCENT / PERCENT_STEP + 1,
        value: format!("{}%", level.percent),
    };
    let volume = slider(p, stops, move |app, stop| {
        if let Some(editor) = &mut app.editor {
            editor.set_level(source, |level| level.percent = stop * PERCENT_STEP);
        }
    })
    .opacity(if level.on { 1.0 } else { 0.4 });
    let mute = row()
        .key(format!("{key}-on"))
        .role("switch")
        .label(label)
        .checked(level.on)
        .focusable(true)
        .on_press(move |app: &mut Ngine| {
            if let Some(editor) = &mut app.editor {
                editor.set_level(source, |level| level.on = !level.on);
            }
        })
        .child(switch(p, &format!("{key}-switch"), level.on));
    let hint = (!level.on).then(|| "Left out of the clip".to_owned());
    let controls = row()
        .key(format!("{key}-controls"))
        .gap(space::S3)
        .center_y()
        .child(volume)
        .child(mute);
    setting(p, key, label, hint, controls)
}

fn sound(p: &Paint, editor: &Editor) -> El {
    let rows = if editor.info.has_separate_sound() {
        vec![
            source_row(p, editor, Source::System),
            source_row(p, editor, Source::Microphone),
        ]
    } else {
        vec![
            caption(
                p,
                "clip-sound-fixed",
                "This recording holds only the mix, so system sound and the microphone cannot be changed apart.",
            )
            .padding(space::S3),
        ]
    };
    section(p, "clip-sound", "Sound in the clip", rows)
}

fn cut_row(p: &Paint, editor: &Editor) -> El {
    let cuts = [Cut::Fast, Cut::Exact];
    let segments = Segments {
        key: "cut",
        label: "Cut",
        choices: vec![Choice::new("fast", "Fast"), Choice::new("exact", "Exact")],
        selected: cuts.iter().position(|cut| *cut == editor.cut),
        segment_width: CUT_WIDTH,
    };
    let hint = match editor.cut {
        Cut::Fast => "Starts at the keyframe before the in point. Nothing is encoded again.",
        Cut::Exact => {
            "Starts and ends on the chosen frames. Only the first moments are encoded again."
        }
    };
    let control = segmented(p, segments, move |app, index| {
        if let (Some(editor), Some(cut)) = (&mut app.editor, cuts.get(index)) {
            editor.cut = *cut;
        }
    });
    setting(p, "cut-row", "Cut", Some(hint.into()), control)
}

fn export_button(p: &Paint, label: &str) -> El {
    button(p, "export", label, Look::Primary, true, Ngine::export_clip)
}

fn progress(p: &Paint, done: f32) -> El {
    el().key("export-progress")
        .role("progressbar")
        .label("Export")
        .range_value(f64::from(done), 0.0, 1.0)
        .size(PROGRESS.0, PROGRESS.1)
        .radius(PROGRESS.1 / 2.0)
        .fill(p.t.control_off)
        .child(
            el().key("export-progress-fill")
                .size((PROGRESS.0 * done).max(PROGRESS.1), PROGRESS.1)
                .radius(PROGRESS.1 / 2.0)
                .fill(p.accent)
                .a11y_hidden(),
        )
}

fn export_row(p: &Paint, app: &Ngine, editor: &Editor) -> El {
    let line = row()
        .key("export-row")
        .min_h(44.0)
        .px(space::S3)
        .py(space::S2)
        .gap(space::S3)
        .center_y();
    match &editor.exporting {
        Exporting::Idle => line
            .child(
                caption(
                    p,
                    "export-words",
                    format!(
                        "One file with one mixed sound track, saved to {}.",
                        app.settings.output_folder.display()
                    ),
                )
                .grow(1.0)
                .min_w(0.0),
            )
            .child(export_button(p, "Export clip")),
        Exporting::Running { done, .. } => line
            .child(progress(p, *done))
            .child(
                text(format!("Exporting {:.0}%", done * 100.0))
                    .key("export-percent")
                    .font(type_::LABEL.features("tnum"))
                    .color(p.t.text),
            )
            .child(spacer())
            .child(button(
                p,
                "export-cancel",
                "Cancel",
                Look::Plain,
                true,
                |app| {
                    if let Some(editor) = &mut app.editor {
                        editor.cancel_export();
                    }
                },
            )),
        Exporting::Done { file, length } => {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let shown = file.clone();
            line.child(
                text(format!("Saved {name}  ·  {}", format::length(*length)))
                    .key("export-done")
                    .role("status")
                    .font(type_::LABEL)
                    .color(p.good)
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            )
            .child(button(
                p,
                "export-show",
                &format!("Show in {}", platform::FILE_MANAGER),
                Look::Plain,
                true,
                move |app| app.reveal(&shown),
            ))
            .child(export_button(p, "Export again"))
        }
        Exporting::Failed(error) => line
            .child(
                text(format!("The clip could not be exported: {error}"))
                    .key("export-failed")
                    .role("alert")
                    .font(type_::LABEL)
                    .color(p.danger)
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            )
            .child(export_button(p, "Try again")),
    }
}

fn empty(p: &Paint) -> El {
    col()
        .key("editor-empty")
        .w_full()
        .h(EMPTY_HEIGHT)
        .center()
        .gap(space::S2)
        .child(
            text("No recording open")
                .key("editor-empty-title")
                .font(HEADING)
                .color(p.t.text),
        )
        .child(caption(
            p,
            "editor-empty-words",
            "Open a recording from the library to preview it, trim it and export a clip.",
        ))
        .child(button(
            p,
            "editor-to-library",
            "Go to the library",
            Look::Plain,
            true,
            |app| app.show(Screen::Library),
        ))
}

fn header(p: &Paint, editor: &Editor) -> El {
    row()
        .key("editor-header")
        .gap(space::S3)
        .center_y()
        .child(
            text(editor.name.clone())
                .key("editor-name")
                .font(HEADING)
                .color(p.t.text)
                .ml(space::S3),
        )
        .child(caption(
            p,
            "editor-details",
            format!(
                "{}  ·  {} × {}",
                format::length(editor.length()),
                editor.info.width,
                editor.info.height
            ),
        ))
        .child(spacer())
        .child(button(
            p,
            "editor-close",
            "Close",
            Look::Plain,
            true,
            Ngine::close_editor,
        ))
}

fn preview(p: &Paint, editor: &Editor) -> El {
    let mut frame = el()
        .key("preview")
        .w_full()
        .h(PREVIEW_HEIGHT)
        .radius(radius::LG)
        .fill(Color::BLACK)
        .center()
        .pixels_fit(&editor.picture, SurfaceFit::Fit);
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

pub fn view(p: &Paint, app: &Ngine, timeline_width: f32) -> El {
    let Some(editor) = &app.editor else {
        return empty(p);
    };
    col()
        .key("editor")
        .gap(space::S3)
        .child(header(p, editor))
        .child(preview(p, editor))
        .child(transport(p, editor))
        .child(timeline(p, editor, timeline_width))
        .child(caption(
            p,
            "editor-keys",
            "Space plays  ·  ← → move one frame, with Shift one second  ·  I and O set the in and out points  ·  drag the ends of the clip",
        ))
        .children(editor.plays_silently().then(|| {
            caption(
                p,
                "preview-silent",
                "No sound output could be opened, so the preview plays without sound. The clip keeps its sound.",
            )
        }))
        .child(sound(p, editor))
        .child(section(
            p,
            "export-section",
            "Export",
            vec![cut_row(p, editor), export_row(p, app, editor)],
        ))
}
