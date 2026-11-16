use rune::tok::{radius, space, type_};
use rune::{El, SurfaceFit, col, el, row, spacer, text, text_input};

use super::widgets::{HEADING, Look, Paint, STRONG, button, caption, card};
use crate::app::Ngine;
use crate::format;
use crate::library::Entry;
use crate::platform;

const THUMBNAIL: (f32, f32) = (128.0, 72.0);
const EMPTY_HEIGHT: f32 = 420.0;
const NAME_HEIGHT: f32 = 26.0;

pub fn details(entry: &Entry) -> String {
    let item = &entry.item;
    let mut parts = Vec::new();
    match &item.info {
        Some(info) => parts.push(format::length(info.length)),
        None => parts.push("Cannot be read as a video".into()),
    }
    parts.push(format::size(item.bytes));
    parts.push(entry.date.clone());
    match item.markers {
        0 => {}
        1 => parts.push("1 marker".into()),
        many => parts.push(format!("{many} markers")),
    }
    if item.unfinished {
        parts.push("cut off, not finished yet".into());
    }
    parts.join("  ·  ")
}

fn total(app: &Ngine) -> String {
    let bytes = app
        .library
        .entries
        .iter()
        .map(|entry| entry.item.bytes)
        .sum();
    let count = match app.library.entries.len() {
        1 => "1 video".to_owned(),
        many => format!("{many} videos"),
    };
    format!("{count}  ·  {}", format::size(bytes))
}

fn name_line(p: &Paint, key: &str, entry: &Entry, renaming: bool, app: &Ngine) -> El {
    if !renaming {
        return text(entry.item.name.clone())
            .key(format!("{key}-name"))
            .font(STRONG)
            .color(p.t.text);
    }
    el().key("rename-box")
        .w_full()
        .h(NAME_HEIGHT)
        .px(space::S2)
        .radius(radius::MD)
        .fill(p.t.control_track)
        .border(1.0, p.accent)
        .center_y()
        .child(
            text_input(&app.rename, type_::BODY)
                .key("rename")
                .label("New name")
                .single_line()
                .color(p.t.text)
                .on_edit(|app: &mut Ngine| &mut app.rename)
                .on_submit(|app: &mut Ngine, tx| {
                    app.commit_rename();
                    tx.blur();
                }),
        )
}

fn actions(p: &Paint, key: &str, index: usize, entry: &Entry, renaming: bool) -> El {
    let line = row().key(format!("{key}-actions")).gap(space::S2);
    if renaming {
        return line
            .child(button(
                p,
                "rename-save",
                "Save name",
                Look::Primary,
                true,
                Ngine::commit_rename,
            ))
            .child(button(
                p,
                "rename-cancel",
                "Cancel",
                Look::Plain,
                true,
                Ngine::cancel_rename,
            ));
    }
    let readable = entry.item.info.is_some();
    line.child(button(
        p,
        &format!("{key}-open"),
        "Open",
        Look::Plain,
        readable,
        move |app| app.open_in_editor(index),
    ))
    .child(button(
        p,
        &format!("{key}-rename"),
        "Rename",
        Look::Plain,
        true,
        move |app| app.start_rename(index),
    ))
    .child(button(
        p,
        &format!("{key}-show"),
        &format!("Show in {}", platform::FILE_MANAGER),
        Look::Plain,
        true,
        move |app| app.reveal_entry(index),
    ))
    .child(button(
        p,
        &format!("{key}-trash"),
        &format!("Move to {}", platform::TRASH),
        Look::Plain,
        true,
        move |app| app.move_to_trash(index),
    ))
}

fn item(p: &Paint, app: &Ngine, index: usize, entry: &Entry) -> El {
    let key = format!("item-{index}");
    let renaming = app.renaming.as_ref() == Some(&entry.item.path);
    let thumbnail = el()
        .key(format!("{key}-thumbnail"))
        .size(THUMBNAIL.0, THUMBNAIL.1)
        .radius(radius::MD)
        .fill(p.t.control_off)
        .pixels_fit(&entry.thumbnail, SurfaceFit::Fill)
        .a11y_hidden();
    row()
        .key(key.clone())
        .padding(space::S2)
        .gap(space::S3)
        .center_y()
        .child(thumbnail)
        .child(
            col()
                .key(format!("{key}-words"))
                .grow(1.0)
                .min_w(0.0)
                .gap(space::S1)
                .child(name_line(p, &key, entry, renaming, app))
                .child(caption(p, &format!("{key}-details"), details(entry)))
                .child(actions(p, &key, index, entry, renaming)),
        )
}

fn empty(p: &Paint, app: &Ngine) -> El {
    let (title, words) = match (&app.library.problem, app.library.loading) {
        (Some(problem), _) => ("The library could not be read", problem.clone()),
        (None, true) => ("Reading the library…", String::new()),
        (None, false) => (
            "No recordings yet",
            format!(
                "Recordings, saved replays and clips appear here. They are kept in {}.",
                app.settings.output_folder.display()
            ),
        ),
    };
    col()
        .key("library-empty")
        .w_full()
        .h(EMPTY_HEIGHT)
        .center()
        .gap(space::S2)
        .child(
            text(title)
                .key("library-empty-title")
                .font(HEADING)
                .color(p.t.text),
        )
        .child(caption(p, "library-empty-words", words))
}

pub fn view(p: &Paint, app: &Ngine) -> El {
    let mut header = row().key("library-header").center_y().gap(space::S3).child(
        text("Library")
            .key("library-title")
            .font(type_::SECTION)
            .color(p.t.text_secondary)
            .ml(space::S3),
    );
    if !app.library.entries.is_empty() {
        header = header.child(caption(p, "library-total", total(app)));
    }
    let header = header.child(spacer()).child(button(
        p,
        "library-folder",
        &format!("Show folder in {}", platform::FILE_MANAGER),
        Look::Plain,
        true,
        |app| {
            let folder = app.settings.output_folder.clone();
            app.reveal(&folder);
        },
    ));
    let list = if app.library.entries.is_empty() {
        empty(p, app)
    } else {
        let rows = app
            .library
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| item(p, app, index, entry))
            .collect();
        card(p, "library-list", rows)
    };
    col()
        .key("library")
        .gap(space::S2)
        .child(header)
        .child(list)
}
