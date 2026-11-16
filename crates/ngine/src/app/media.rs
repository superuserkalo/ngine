use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ngine_recorder::rename;
use rune::{Key, KeyEvent, TextEdit};

use super::{Ngine, Popup, Screen};
use crate::editor::{Editor, News};
use crate::format;
use crate::library::Library;
use crate::platform;

const CLIP_NAME: &str = "Clip";

impl Ngine {
    /// Lists the output folder from the disk, with thumbnails kept in
    /// `thumbnails`.
    pub fn use_library_on_disk(&mut self, thumbnails: PathBuf) {
        self.library = Library::on_disk(thumbnails);
        self.refresh_library();
    }

    /// Lists a folder from the disk and waits for it, for a headless picture
    /// of the real library.
    pub fn show_library_of(&mut self, folder: &Path) {
        folder.clone_into(&mut self.settings.output_folder);
        self.library = Library::on_disk(std::env::temp_dir().join("ngine-tooling-thumbnails"));
        self.library.load_now(folder);
        self.screen = Screen::Library;
    }

    /// Opens a video in the clip editor and waits for its first picture, for a
    /// headless picture of the real editor.
    pub fn show_editor_on(&mut self, file: &Path, at: Duration) {
        const LONGEST_WAIT: Duration = Duration::from_secs(20);
        match Editor::open(file) {
            Ok(mut editor) => {
                editor.wait_for_the_picture(LONGEST_WAIT);
                editor.seek(at);
                editor.wait_for_the_picture(LONGEST_WAIT);
                self.editor = Some(editor);
            }
            Err(error) => self.tell(format!("This video could not be opened: {error}"), true),
        }
        self.screen = Screen::Editor;
    }

    pub fn refresh_library(&mut self) {
        self.library.refresh(&self.settings.output_folder);
    }

    pub fn show(&mut self, screen: Screen) {
        if self.screen == Screen::Editor
            && screen != Screen::Editor
            && let Some(editor) = &mut self.editor
        {
            editor.pause();
        }
        self.screen = screen;
        self.renaming = None;
        self.popup = None;
        self.capture = None;
        match screen {
            Screen::Library => self.refresh_library(),
            Screen::Capture | Screen::Editor => {}
        }
    }

    pub fn open_in_editor(&mut self, index: usize) {
        let Some(file) = self.library.path(index) else {
            return;
        };
        self.editor = None;
        self.clip_name = TextEdit::new("");
        match Editor::open(&file) {
            Ok(editor) => {
                self.editor = Some(editor);
                self.show(Screen::Editor);
            }
            Err(error) => self.tell(format!("This video could not be opened: {error}"), true),
        }
    }

    /// Closes the clip editor and goes back to the library it was opened from.
    pub fn close_editor(&mut self) {
        self.editor = None;
        if self.screen == Screen::Editor {
            self.show(Screen::Library);
        }
    }

    pub fn start_rename(&mut self, index: usize) {
        let Some(entry) = self.library.entries.get(index) else {
            return;
        };
        self.rename = TextEdit::new(entry.item.name.clone());
        self.rename.set_selection(0, entry.item.name.len());
        self.rename.focused = true;
        self.renaming = Some(entry.item.path.clone());
        self.popup = None;
    }

    pub fn cancel_rename(&mut self) {
        self.renaming = None;
    }

    pub fn commit_rename(&mut self) {
        let Some(file) = self.renaming.take() else {
            return;
        };
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.file == file)
        {
            self.editor = None;
        }
        match rename(&file, &self.rename.text) {
            Ok(renamed) => self.library.renamed(&file, renamed),
            Err(error) => {
                self.renaming = Some(file);
                self.tell(error.to_string(), true);
            }
        }
    }

    pub fn reveal(&mut self, file: &Path) {
        if let Err(error) = platform::reveal(file) {
            self.tell(
                format!(
                    "The {} could not be opened: {error}",
                    platform::FILE_MANAGER
                ),
                true,
            );
        }
    }

    pub fn open_folder(&mut self, folder: &Path) {
        if let Err(error) = platform::open_folder(folder) {
            self.tell(format!("The folder could not be opened: {error}"), true);
        }
    }

    pub fn export_clip(&mut self) {
        let folder = self.settings.output_folder.clone();
        let typed = format::file_name(&self.clip_name.text);
        let name = if typed.is_empty() {
            format::stamped(CLIP_NAME, &format::local(SystemTime::now()))
        } else {
            typed
        };
        if let Some(editor) = &mut self.editor {
            editor.pause();
            editor.export(&folder, &name);
        }
    }

    pub(super) fn follow_media(&mut self) -> bool {
        let mut changed = self.library.poll();
        for line in self.library.take_news() {
            self.tell(line, false);
        }
        let Some(editor) = &mut self.editor else {
            return changed;
        };
        let (moved, news) = editor.tick();
        changed |= moved;
        match news {
            Some(News::Exported(length)) => {
                self.confirm(format!("Clip saved · {}", format::length(length)));
                self.refresh_library();
                changed = true;
            }
            Some(News::ExportFailed) => changed = true,
            None => {}
        }
        changed
    }

    pub(super) fn media_wants_ticks(&self) -> Option<Duration> {
        const FRAME: Duration = Duration::from_millis(16);
        const WAITING: Duration = Duration::from_millis(100);
        if self.editor.as_ref().is_some_and(Editor::wants_ticks) {
            Some(FRAME)
        } else if self.library.is_busy() {
            Some(WAITING)
        } else {
            None
        }
    }

    /// Fills the window with the video, the controls over it. With
    /// `take_the_screen`, the window goes full screen too and comes back
    /// when watching ends.
    pub fn watch_full_screen(&mut self, take_the_screen: bool) {
        let clock = self.clock;
        if let Some(editor) = &mut self.editor {
            editor.watching = Some(crate::editor::Watching {
                moved_at: clock,
                on_controls: false,
                took_the_screen: take_the_screen,
            });
        }
        self.popup = None;
    }

    /// Ends full screen watching. Returns whether ngine had taken the window
    /// full screen, so the caller gives it back.
    pub fn stop_watching(&mut self) -> bool {
        self.editor
            .as_mut()
            .and_then(|editor| editor.watching.take())
            .is_some_and(|watching| watching.took_the_screen)
    }

    pub fn watching(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.watching.is_some())
    }

    /// The pointer moved over the video watched full screen.
    pub fn pointer_moved(&mut self, on_controls: bool) {
        let clock = self.clock;
        if let Some(watching) = self
            .editor
            .as_mut()
            .and_then(|editor| editor.watching.as_mut())
        {
            watching.moved_at = clock;
            watching.on_controls = on_controls;
        }
    }

    /// The clip editor's own keys. Returns whether the key was one of them.
    pub fn editor_key(&mut self, event: &KeyEvent) -> bool {
        let Some(editor) = &mut self.editor else {
            return false;
        };
        let far = event.mods.shift;
        match &event.key {
            Key::Text(key) if key == " " => editor.toggle_play(),
            Key::Text(key) if key == "?" => {
                self.popup = match self.popup {
                    Some(Popup::EditorKeys) => None,
                    _ => Some(Popup::EditorKeys),
                };
            }
            Key::Left if far => editor.skip(-1),
            Key::Right if far => editor.skip(1),
            Key::Left => editor.step(-1),
            Key::Right => editor.step(1),
            Key::Text(key) if key.eq_ignore_ascii_case("i") => editor.set_start(editor.position),
            Key::Text(key) if key.eq_ignore_ascii_case("o") => editor.set_end(editor.position),
            Key::Home => editor.seek(editor.start),
            Key::End => editor.seek(editor.end),
            _ => return false,
        }
        true
    }
}
