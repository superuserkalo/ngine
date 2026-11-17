use rune::tok::{radius, space, type_};
use rune::{Align, Color, Cursor, El, Side, SurfaceFit, col, el, row, spacer, text, text_input};

use super::widgets::{
    Button, CAPTION, CONTROL, Choice, Field, Floating, Look, MenuItem, NUMBERS, Option_, Paint,
    Segments, Select, dialog, empty_state, field, float, float_at, menu, menu_rule, segmented,
    select, tag,
};
use super::{Ui, icons, page, sheet};
use crate::app::browse::{Filter, Sort, counted};
use crate::app::{Ngine, Popup, Screen};
use crate::format;
use crate::library::Entry;
use crate::platform;

const SMALLEST_CARD: f32 = 200.0;
const GRID_GAP: f32 = 12.0;
const FILTER_WIDTH: f32 = 96.0;
const NARROW_FILTER_WIDTH: f32 = 80.0;
const SORT_WIDTH: f32 = 136.0;
const NARROW_SORT_WIDTH: f32 = 120.0;
const NARROW_TOOLBAR_BELOW: f32 = 720.0;
const SEARCH_WIDTH: f32 = 96.0;
const NAME_HEIGHT: f32 = 20.0;
const SELECTION_NEEDS: f32 = 360.0;
const SELECTED_RING: f32 = 2.0;

pub const UNREADABLE: &str = "This file cannot be read as a video";

fn total(app: &Ngine, shown: &[usize]) -> String {
    let bytes = shown
        .iter()
        .filter_map(|index| app.library.entries.get(*index))
        .map(|entry| entry.item.bytes)
        .sum();
    format!("{} · {}", counted(shown.len()), format::size(bytes))
}

/// What a card says under the name: when, and how large.
pub fn details(entry: &Entry) -> String {
    format!("{} · {}", entry.date, format::size(entry.item.bytes))
}

fn toolbar(ui: &mut Ui<'_>) -> (El, f32) {
    let p = ui.p;
    let app = ui.app;
    let narrow = ui.content_width() < NARROW_TOOLBAR_BELOW;
    let (filter_width, sort_width) = if narrow {
        (NARROW_FILTER_WIDTH, NARROW_SORT_WIDTH)
    } else {
        (FILTER_WIDTH, SORT_WIDTH)
    };
    let segments = Segments {
        key: "library-filter",
        label: "Show",
        choices: Filter::ALL
            .iter()
            .map(|filter| Choice::new(filter.id(), filter.label()))
            .collect(),
        selected: Filter::ALL.iter().position(|one| *one == app.browse.filter),
        segment_width: filter_width,
    };
    let filter = segmented(&p, segments, |app, index| {
        if let Some(filter) = Filter::ALL.get(index) {
            app.set_filter(*filter);
        }
    });
    let search = field(
        &p,
        &Field {
            key: "library-search",
            label: "Search by name",
            edit: &app.browse.search,
            placeholder: "Search",
            icon: Some(&icons::SEARCH),
        },
        |app| &mut app.browse.search,
        Ngine::forget_hidden_selection,
    )
    .grow(1.0)
    .min_w(SEARCH_WIDTH);
    let sort = Select {
        key: "library-sort",
        label: "Order",
        options: Sort::ALL
            .iter()
            .map(|sort| Option_::new(sort.label()))
            .collect(),
        selected: Sort::ALL.iter().position(|one| *one == app.browse.sort),
        width: sort_width,
        align: Align::End,
    };
    let sort = select(&p, app, &mut ui.layer, sort, |app, index| {
        if let Some(sort) = Sort::ALL.get(index) {
            app.set_sort(*sort);
        }
    });
    let filters = f32::from(u8::try_from(Filter::ALL.len()).unwrap_or(u8::MAX));
    let needs =
        filters * filter_width + 4.0 + SEARCH_WIDTH + sort_width + CONTROL + 3.0 * space::S2;
    let bar = row()
        .key("library-toolbar")
        .gap(space::S2)
        .center_y()
        .flex_wrap()
        .child(filter)
        .child(search)
        .child(sort)
        .child(folder_button(&p, app));
    (bar, needs)
}

fn selection_bar(p: &Paint, app: &Ngine) -> Option<El> {
    let count = app.browse.selected.len();
    (count > 0).then(|| {
        row()
            .key("selection-bar")
            .h(CONTROL)
            .gap(space::S2)
            .center_y()
            .child(
                text(format!("{count} selected"))
                    .key("selection-count")
                    .font(type_::LABEL)
                    .color(p.t.text),
            )
            .child(spacer())
            .child(
                Button::new("selection-clear", "Clear selection")
                    .look(Look::Quiet)
                    .build(p, Ngine::clear_selection),
            )
            .child(
                Button::new("selection-trash", &format!("Move to {}", platform::TRASH))
                    .icon(&icons::TRASH_2)
                    .look(Look::Danger)
                    .build(p, Ngine::ask_to_trash_selection),
            )
    })
}

fn badge(key: &str, words: String) -> El {
    el().key(format!("{key}-badge"))
        .h(18.0)
        .px(5.0)
        .center()
        .radius(4.0)
        .fill(Color::BLACK.alpha(0.72))
        .child(text(words).key(key).font(NUMBERS).color(Color::WHITE))
}

fn name_line(p: &Paint, app: &Ngine, key: &str, entry: &Entry) -> El {
    if app.renaming.as_ref() != Some(&entry.item.path) {
        return row()
            .key(format!("{key}-name-line"))
            .h(NAME_HEIGHT)
            .center_y()
            .child(
                text(entry.item.name.clone())
                    .key(format!("{key}-name"))
                    .font(type_::LABEL)
                    .color(p.t.text)
                    .truncate()
                    .min_w(0.0)
                    .a11y_hidden(),
            );
    }
    el().key(crate::app::RENAME_BOX)
        .w_full()
        .h(NAME_HEIGHT)
        .px(space::S1 + 2.0)
        .radius(radius::SM)
        .fill(p.t.control_track)
        .border(1.0, p.accent)
        .center_y()
        .child(
            text_input(&app.rename, type_::LABEL)
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

fn tags(p: &Paint, key: &str, entry: &Entry) -> El {
    let mut tags = row()
        .key(format!("{key}-tags"))
        .gap(space::S1 + 2.0)
        .center_y();
    if entry.item.markers > 0 {
        let markers = match entry.item.markers {
            1 => "1 marker".to_owned(),
            many => format!("{many} markers"),
        };
        tags = tags.child(
            text(markers)
                .key(format!("{key}-markers"))
                .font(CAPTION)
                .color(p.faint),
        );
    }
    if entry.item.unfinished {
        tags = tags.child(tag(
            p,
            &format!("{key}-unfinished"),
            "Unfinished",
            p.warning,
        ));
    }
    if entry.item.info.is_none() {
        tags = tags.child(tag(p, &format!("{key}-unreadable"), "Unreadable", p.danger));
    }
    tags
}

fn card(p: &Paint, app: &Ngine, index: usize, entry: &Entry, width: f32) -> El {
    let key = format!("item-{index}");
    let selected = app.is_selected(index);
    let mut corner = col()
        .key(format!("{key}-thumbnail"))
        .w_full()
        .h((width - 2.0 * space::S1) * 9.0 / 16.0)
        .radius(radius::MD)
        .fill(Color::BLACK)
        .pixels_fit(&entry.thumbnail, SurfaceFit::Fill)
        .border(
            SELECTED_RING,
            if selected {
                p.accent
            } else {
                Color::TRANSPARENT
            },
        )
        .padding(space::S1 + 2.0)
        .justify_end()
        .items_end();
    if let Some(info) = &entry.item.info {
        corner = corner.child(badge(&format!("{key}-length"), format::timer(info.length)));
    }
    let tags = tags(p, &key, entry);
    let more = Button::new(&format!("{key}-more"), "More actions")
        .only_icon(&icons::ELLIPSIS)
        .look(Look::Quiet)
        .opens(matches!(
            &app.popup,
            Some(Popup::ItemMenu { file, at: None }) if *file == entry.item.path
        ))
        .build(p, move |app| app.open_item_menu(index, None));
    let words = row()
        .key(format!("{key}-words"))
        .gap(space::S1)
        .px(2.0)
        .child(
            col()
                .key(format!("{key}-text"))
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(name_line(p, app, &key, entry))
                .child(
                    text(details(entry))
                        .key(format!("{key}-details"))
                        .font(CAPTION)
                        .color(p.t.text_secondary)
                        .truncate()
                        .a11y_hidden(),
                ),
        )
        .child(more);
    col()
        .key(key)
        .role("button")
        .label(entry.item.name.clone())
        .checked(selected)
        .w(width)
        .padding(space::S1)
        .gap(space::S2)
        .radius(radius::LG)
        .hover(p.t.hover)
        .press(p.t.press)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |app: &mut Ngine, tx| app.press_item(index, tx.mods()))
        .on_secondary_press(move |app: &mut Ngine, at, _| {
            app.open_item_menu(index, Some((at.x, at.y)));
        })
        .child(corner)
        .child(words)
        .child(tags.px(2.0).pb(space::S1))
}

fn grid(ui: &Ui<'_>, shown: &[usize]) -> El {
    let available = ui.content_width();
    let columns = ((available + GRID_GAP) / (SMALLEST_CARD + GRID_GAP))
        .floor()
        .max(1.0);
    let width = ((available - GRID_GAP * (columns - 1.0)) / columns).floor();
    let mut grid = row()
        .key("library-grid")
        .role("list")
        .label("Videos")
        .w_full()
        .gap(GRID_GAP)
        .flex_wrap();
    for index in shown {
        if let Some(entry) = ui.app.library.entries.get(*index) {
            grid = grid.child(card(&ui.p, ui.app, *index, entry, width));
        }
    }
    grid
}

fn item_menu(ui: &mut Ui<'_>) {
    let p = ui.p;
    let Some(Popup::ItemMenu { file, at }) = &ui.app.popup else {
        return;
    };
    let Some(index) = ui.app.index_of(file) else {
        return;
    };
    let readable = ui
        .app
        .library
        .entries
        .get(index)
        .is_some_and(|entry| entry.item.info.is_some());
    let trashed = file.clone();
    let shown = file.clone();
    let items = vec![
        MenuItem::new("menu-open", "Open")
            .icon(&icons::PLAY)
            .detail("Enter")
            .blocked((!readable).then(|| UNREADABLE.to_owned()))
            .build(&p, "menuitem", move |app, _| {
                app.close_popup();
                app.open_in_editor(index);
            }),
        MenuItem::new("menu-rename", "Rename")
            .icon(&icons::PENCIL)
            .detail("F2")
            .build(&p, "menuitem", move |app, tx| {
                app.start_rename(index);
                tx.scroll_into_view(crate::app::RENAME_BOX);
            }),
        MenuItem::new("menu-show", &format!("Show in {}", platform::FILE_MANAGER))
            .icon(&icons::FOLDER_OPEN)
            .build(&p, "menuitem", move |app, _| {
                app.close_popup();
                app.reveal(&shown);
            }),
        menu_rule(&p, "menu-rule"),
        MenuItem::new("menu-trash", &format!("Move to {}", platform::TRASH))
            .icon(&icons::TRASH_2)
            .detail("Delete")
            .danger()
            .build(&p, "menuitem", move |app, _| {
                app.ask_to_trash(vec![trashed.clone()]);
            }),
    ];
    let surface = menu(&p, "item-menu", "Video actions", items);
    if let Some(at) = at {
        float_at(&mut ui.layer, "item-menu", *at, ui.window, surface);
    } else {
        let anchor = format!("item-{index}-more");
        let floating = Floating {
            key: "item-menu",
            anchor: &anchor,
            side: Side::Bottom,
            align: Align::End,
        };
        float(&mut ui.layer, &floating, surface);
    }
}

fn trash_question(ui: &mut Ui<'_>) {
    let p = ui.p;
    let Some(Popup::ConfirmTrash(files)) = &ui.app.popup else {
        return;
    };
    let title = match files.as_slice() {
        [one] => format!(
            "Move “{}” to the {}?",
            one.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            platform::TRASH
        ),
        many => format!("Move {} to the {}?", counted(many.len()), platform::TRASH),
    };
    let actions = vec![
        Button::new("trash-cancel", "Cancel").build(&p, Ngine::close_popup),
        Button::new("trash-confirm", &format!("Move to {}", platform::TRASH))
            .look(Look::Danger)
            .build(&p, Ngine::trash_confirmed),
    ];
    dialog(
        &p,
        &mut ui.layer,
        "trash-ask",
        &title,
        format!("You can put them back from the {}.", platform::TRASH),
        actions,
    );
}

fn empty(ui: &Ui<'_>) -> El {
    let p = &ui.p;
    let app = ui.app;
    if let Some(problem) = &app.library.problem {
        return empty_state(
            p,
            "library-empty",
            &icons::TRIANGLE_ALERT,
            "The library could not be read",
            problem.clone(),
            Some(Button::new("library-empty-retry", "Try again").build(p, Ngine::refresh_library)),
        );
    }
    if app.library.loading {
        return empty_state(
            p,
            "library-empty",
            &icons::IMAGES,
            "Reading the library…",
            String::new(),
            None,
        );
    }
    empty_state(
        p,
        "library-empty",
        &icons::FILM,
        "No recordings yet",
        format!(
            "Recordings, saved replays and clips appear here. They are kept in {}.",
            app.settings.output_folder.display()
        ),
        Some(
            Button::new("library-empty-capture", "Go to Capture")
                .look(Look::Primary)
                .build(p, |app| app.show(Screen::Capture)),
        ),
    )
}

fn nothing_found(p: &Paint, app: &Ngine) -> El {
    let searched = app.browse.search.text.trim().to_owned();
    let filter = app.browse.filter;
    let (words, button) = match (searched.is_empty(), filter) {
        (true, filter) => (
            format!("There are no {} yet.", filter.label().to_lowercase()),
            "Show all",
        ),
        (false, Filter::All) => (
            format!("No video has “{searched}” in its name."),
            "Clear search",
        ),
        (false, filter) => (
            format!(
                "No {} have “{searched}” in their name.",
                filter.label().to_lowercase()
            ),
            "Show all",
        ),
    };
    empty_state(
        p,
        "library-none-found",
        &icons::SEARCH,
        "Nothing matches",
        words,
        Some(Button::new("library-show-all", button).build(p, |app| {
            app.clear_search();
            app.set_filter(Filter::All);
        })),
    )
}

fn folder_button(p: &Paint, app: &Ngine) -> El {
    Button::new("library-folder", "Open folder")
        .only_icon(&icons::FOLDER_OPEN)
        .look(Look::Quiet)
        .help(format!(
            "Open {} in {}",
            app.settings.output_folder.display(),
            platform::FILE_MANAGER
        ))
        .build(p, |app| {
            let folder = app.settings.output_folder.clone();
            app.open_folder(&folder);
        })
}

pub fn view(ui: &mut Ui<'_>) -> El {
    let p = ui.p;
    let app = ui.app;
    let first = if app.library.entries.is_empty() {
        None
    } else if let Some(bar) = selection_bar(&p, app) {
        ui.top_row(bar, SELECTION_NEEDS)
    } else {
        let (bar, needs) = toolbar(ui);
        ui.top_row(bar, needs)
    };
    let mut content = sheet(ui, "library").gap(space::S3 + space::S1);
    if app.library.entries.is_empty() {
        content = content.child(empty(ui));
    } else {
        if let Some(bar) = first {
            content = content.child(bar);
        }
        let shown = app.shown();
        content = if shown.is_empty() {
            content.child(nothing_found(&p, app))
        } else {
            content.child(grid(ui, &shown)).child(
                text(total(app, &shown))
                    .key("library-total")
                    .font(CAPTION)
                    .color(p.faint),
            )
        };
    }
    item_menu(ui);
    trash_question(ui);
    page("library", content)
}
