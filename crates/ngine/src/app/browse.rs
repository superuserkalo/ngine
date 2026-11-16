use std::cmp::Reverse;
use std::path::PathBuf;
use std::time::Duration;

use rune::{Key, KeyEvent, Mods, TextEdit};

use super::{Ngine, Popup};
use crate::library::Entry;
use crate::platform;

/// What a video in the library is, read from the name ngine gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Recording,
    Replay,
    Clip,
}

impl Kind {
    pub fn of(name: &str) -> Self {
        if name.starts_with("Replay") {
            Self::Replay
        } else if name.starts_with("Clip") {
            Self::Clip
        } else {
            Self::Recording
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Recordings,
    Replays,
    Clips,
}

impl Filter {
    pub const ALL: [Self; 4] = [Self::All, Self::Recordings, Self::Replays, Self::Clips];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Recordings => "Recordings",
            Self::Replays => "Replays",
            Self::Clips => "Clips",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Recordings => "recordings",
            Self::Replays => "replays",
            Self::Clips => "clips",
        }
    }

    fn admits(self, kind: Kind) -> bool {
        match self {
            Self::All => true,
            Self::Recordings => kind == Kind::Recording,
            Self::Replays => kind == Kind::Replay,
            Self::Clips => kind == Kind::Clip,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Largest,
    Longest,
}

impl Sort {
    pub const ALL: [Self; 4] = [Self::Newest, Self::Oldest, Self::Largest, Self::Longest];

    pub fn label(self) -> &'static str {
        match self {
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::Largest => "Largest first",
            Self::Longest => "Longest first",
        }
    }
}

/// How the library is looked at: which videos, in what order, and which of
/// them are selected.
#[derive(Default)]
pub struct Browse {
    pub filter: Filter,
    pub sort: Sort,
    pub search: TextEdit,
    pub selected: Vec<PathBuf>,
    anchor: Option<PathBuf>,
}

fn length_of(entry: &Entry) -> Duration {
    entry
        .item
        .info
        .as_ref()
        .map_or(Duration::ZERO, |info| info.length)
}

/// The entries the filter and the search let through, in the chosen order, as
/// positions in `entries`.
pub fn shown(entries: &[Entry], browse: &Browse) -> Vec<usize> {
    let words: Vec<String> = browse
        .search
        .text
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let mut found: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| browse.filter.admits(Kind::of(&entry.item.name)))
        .filter(|(_, entry)| {
            let name = entry.item.name.to_lowercase();
            words.iter().all(|word| name.contains(word))
        })
        .map(|(index, _)| index)
        .collect();
    match browse.sort {
        Sort::Newest => found.sort_by_key(|index| Reverse(entries[*index].item.modified)),
        Sort::Oldest => found.sort_by_key(|index| entries[*index].item.modified),
        Sort::Largest => found.sort_by_key(|index| Reverse(entries[*index].item.bytes)),
        Sort::Longest => found.sort_by_key(|index| Reverse(length_of(&entries[*index]))),
    }
    found
}

pub fn counted(count: usize) -> String {
    match count {
        1 => "1 video".to_owned(),
        many => format!("{many} videos"),
    }
}

fn item_index(focused: Option<&str>) -> Option<usize> {
    focused?.strip_prefix("item-")?.parse().ok()
}

impl Ngine {
    pub fn shown(&self) -> Vec<usize> {
        shown(&self.library.entries, &self.browse)
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.library
            .entries
            .get(index)
            .is_some_and(|entry| self.browse.selected.contains(&entry.item.path))
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.browse.filter = filter;
        self.forget_hidden_selection();
    }

    pub fn set_sort(&mut self, sort: Sort) {
        self.browse.sort = sort;
        self.popup = None;
    }

    pub fn clear_search(&mut self) {
        self.browse.search = TextEdit::new("");
    }

    pub fn forget_hidden_selection(&mut self) {
        let visible: Vec<PathBuf> = self
            .shown()
            .into_iter()
            .filter_map(|index| self.library.path(index))
            .collect();
        self.browse.selected.retain(|file| visible.contains(file));
    }

    pub fn toggle_selected(&mut self, index: usize) {
        let Some(file) = self.library.path(index) else {
            return;
        };
        if let Some(at) = self.browse.selected.iter().position(|one| *one == file) {
            self.browse.selected.remove(at);
        } else {
            self.browse.selected.push(file.clone());
        }
        self.browse.anchor = Some(file);
    }

    fn select_through(&mut self, index: usize) {
        let shown = self.shown();
        let anchor = self
            .browse
            .anchor
            .as_ref()
            .and_then(|anchor| {
                shown
                    .iter()
                    .position(|one| self.library.path(*one).as_ref() == Some(anchor))
            })
            .unwrap_or(0);
        let Some(to) = shown.iter().position(|one| *one == index) else {
            return;
        };
        let (from, to) = (anchor.min(to), anchor.max(to));
        self.browse.selected = shown[from..=to]
            .iter()
            .filter_map(|one| self.library.path(*one))
            .collect();
    }

    pub fn select_all_shown(&mut self) {
        self.browse.selected = self
            .shown()
            .into_iter()
            .filter_map(|index| self.library.path(index))
            .collect();
    }

    pub fn clear_selection(&mut self) {
        self.browse.selected.clear();
        self.browse.anchor = None;
    }

    /// A press on a video: plain opens it, with Cmd or Ctrl it joins or
    /// leaves the selection, with Shift the selection runs up to it.
    pub fn press_item(&mut self, index: usize, mods: Mods) {
        if mods.shift {
            self.select_through(index);
        } else if mods.cmd || mods.ctrl {
            self.toggle_selected(index);
        } else {
            self.clear_selection();
            self.open_in_editor(index);
        }
    }

    pub fn open_item_menu(&mut self, index: usize, at: Option<(f32, f32)>) {
        if let Some(file) = self.library.path(index) {
            self.popup = Some(Popup::ItemMenu { file, at });
        }
    }

    pub fn index_of(&self, file: &std::path::Path) -> Option<usize> {
        self.library
            .entries
            .iter()
            .position(|entry| entry.item.path == file)
    }

    /// Asks before moving videos to the trash. Nothing is asked for nothing.
    pub fn ask_to_trash(&mut self, files: Vec<PathBuf>) {
        self.popup = (!files.is_empty()).then_some(Popup::ConfirmTrash(files));
    }

    pub fn ask_to_trash_selection(&mut self) {
        self.ask_to_trash(self.browse.selected.clone());
    }

    /// Moves the videos the open question names to the trash.
    pub fn trash_confirmed(&mut self) {
        let Some(Popup::ConfirmTrash(files)) = self.popup.take() else {
            return;
        };
        let mut moved = 0;
        let mut failure = None;
        for file in &files {
            if self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.file == *file)
            {
                self.editor = None;
            }
            match (self.trash)(file) {
                Ok(()) => {
                    self.library.forget(file);
                    moved += 1;
                }
                Err(error) => failure = Some(error),
            }
        }
        self.browse.selected.retain(|file| !files.contains(file));
        match failure {
            Some(error) => self.tell(
                format!(
                    "Not everything could be moved to the {}: {error}",
                    platform::TRASH
                ),
                true,
            ),
            None => self.tell(
                format!("{} moved to the {}", counted(moved), platform::TRASH),
                false,
            ),
        }
    }

    /// The library's own keys. `focused` is the key of the focused element.
    /// Returns whether the key was one of them.
    pub fn library_key(&mut self, event: &KeyEvent, focused: Option<&str>) -> bool {
        if focused.is_some_and(|key| !key.starts_with("item-")) {
            return false;
        }
        let current = item_index(focused);
        match &event.key {
            Key::Delete | Key::Backspace => {
                let files = if self.browse.selected.is_empty() {
                    current
                        .and_then(|index| self.library.path(index))
                        .into_iter()
                        .collect()
                } else {
                    self.browse.selected.clone()
                };
                self.ask_to_trash(files);
            }
            Key::Function(2) => {
                let only_selected = match self.browse.selected.as_slice() {
                    [one] => self.index_of(one),
                    _ => None,
                };
                match current.or(only_selected) {
                    Some(index) => self.start_rename(index),
                    None => return false,
                }
            }
            Key::Command('a') => self.select_all_shown(),
            Key::Escape if !self.browse.selected.is_empty() => self.clear_selection(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Library;
    use std::path::Path;

    fn names(library: &Library, browse: &Browse) -> Vec<String> {
        shown(&library.entries, browse)
            .into_iter()
            .map(|index| library.entries[index].item.name.clone())
            .collect()
    }

    #[test]
    fn a_video_is_a_replay_or_a_clip_by_the_name_ngine_gave_it() {
        assert_eq!(Kind::of("Replay 2026-11-16 13.41.07"), Kind::Replay);
        assert_eq!(Kind::of("Clip 2026-11-15 22.18.40"), Kind::Clip);
        assert_eq!(Kind::of("Recording 2026-11-16 14.03.22"), Kind::Recording);
        assert_eq!(Kind::of("Boss fight"), Kind::Recording);
    }

    #[test]
    fn the_filter_keeps_one_kind() {
        let library = Library::sample(Path::new("/videos"));
        let mut browse = Browse::default();
        assert_eq!(names(&library, &browse).len(), 5);
        browse.filter = Filter::Replays;
        assert_eq!(names(&library, &browse), ["Replay 2026-11-16 13.41.07"]);
        browse.filter = Filter::Clips;
        assert_eq!(names(&library, &browse), ["Clip 2026-11-15 22.18.40"]);
        browse.filter = Filter::Recordings;
        assert_eq!(names(&library, &browse).len(), 3);
    }

    #[test]
    fn the_search_finds_every_word_whatever_its_case() {
        let library = Library::sample(Path::new("/videos"));
        let mut browse = Browse {
            search: TextEdit::new("FIGHT boss"),
            ..Browse::default()
        };
        assert_eq!(names(&library, &browse), ["Boss fight"]);
        browse.search = TextEdit::new("2026-11-16");
        assert_eq!(names(&library, &browse).len(), 2);
        browse.search = TextEdit::new("nothing like this");
        assert_eq!(names(&library, &browse), Vec::<String>::new());
    }

    #[test]
    fn the_order_follows_the_date_the_size_or_the_length() {
        let library = Library::sample(Path::new("/videos"));
        let mut browse = Browse::default();
        assert_eq!(names(&library, &browse)[0], "Recording 2026-11-16 14.03.22");
        browse.sort = Sort::Oldest;
        assert_eq!(names(&library, &browse)[0], "Recording 2026-11-12 09.15.00");
        browse.sort = Sort::Largest;
        assert_eq!(names(&library, &browse)[0], "Boss fight");
        browse.sort = Sort::Longest;
        assert_eq!(
            names(&library, &browse)[..2],
            ["Boss fight", "Recording 2026-11-16 14.03.22"]
        );
    }
}
