use crate::complete::Completer;
use crate::{Editor, Event, EventKind};
use sl_console::event::{Key, KeyCode, KeyMod};
use std::io::{self, ErrorKind};

pub trait KeyMap {
    //: Default {
    fn handle_key_core<'a>(&mut self, key: Key, editor: &mut Editor<'a>) -> io::Result<()>;

    fn init<'a>(&mut self, _editor: &mut Editor<'a>) {}

    fn handle_key<'a>(
        &mut self,
        mut key: Key,
        editor: &mut Editor<'a>,
        handler: &mut dyn Completer,
    ) -> io::Result<bool> {
        let mut done = false;

        handler.on_event(Event::new(editor, EventKind::BeforeKey(key)));

        let is_empty = editor.current_buffer().is_empty();

        if key.code == KeyCode::Char('h') && key.mods == Some(KeyMod::Ctrl) {
            // XXX: Might need to change this when remappable keybindings are added.
            key = Key::new(KeyCode::Backspace);
        }

        match (key.code, key.mods) {
            (KeyCode::Char('c'), Some(KeyMod::Ctrl)) => {
                editor.handle_newline()?;
                return Err(io::Error::new(ErrorKind::Interrupted, "ctrl-c"));
            }
            // if the current buffer is empty, treat ctrl-d as eof
            (KeyCode::Char('d'), Some(KeyMod::Ctrl)) if is_empty => {
                editor.handle_newline()?;
                return Err(io::Error::new(ErrorKind::UnexpectedEof, "ctrl-d"));
            }
            (KeyCode::Char('\t'), None) => editor.complete(handler)?,
            (KeyCode::Char('\n'), None) => {
                done = editor.handle_newline()?;
            }
            (KeyCode::Char('f'), Some(KeyMod::Ctrl))
                if editor.is_currently_showing_autosuggestion() =>
            {
                editor.accept_autosuggestion()?;
            }
            (KeyCode::Char('r'), Some(KeyMod::Ctrl)) => {
                editor.search(false)?;
            }
            (KeyCode::Char('s'), Some(KeyMod::Ctrl)) => {
                editor.search(true)?;
            }
            (KeyCode::Right, None)
                if editor.is_currently_showing_autosuggestion()
                    && editor.cursor_is_at_end_of_line() =>
            {
                editor.accept_autosuggestion()?;
            }
            _ => {
                self.handle_key_core(key, editor)?;
                editor.skip_completions_hint();
            }
        };

        handler.on_event(Event::new(editor, EventKind::AfterKey(key)));

        editor.flush()?;

        Ok(done)
    }
}

pub mod vi;
pub use vi::Vi;

pub mod emacs;
pub use emacs::Emacs;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::get_buffer_words;
    use crate::{History, Prompt};
    use sl_console::event::Key;
    use std::io::ErrorKind;

    #[derive(Default)]
    struct TestKeyMap;

    impl KeyMap for TestKeyMap {
        fn handle_key_core<'a>(&mut self, _: Key, _: &mut Editor<'a>) -> io::Result<()> {
            Ok(())
        }
    }

    struct EmptyCompleter;

    impl Completer for EmptyCompleter {
        fn completions(&mut self, _start: &str) -> Vec<String> {
            Vec::default()
        }
    }

    #[test]
    /// when the current buffer is empty, ctrl-d generates and eof error
    fn ctrl_d_empty() {
        let mut out = Vec::new();
        let mut history = History::new();
        let words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &words,
            &mut buf,
        )
        .unwrap();
        let mut map = TestKeyMap;

        let res = map.handle_key(
            Key::new_mod(KeyCode::Char('d'), KeyMod::Ctrl),
            &mut ed,
            &mut EmptyCompleter,
        );
        assert_eq!(res.is_err(), true);
        assert_eq!(res.err().unwrap().kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    /// when the current buffer is not empty, ctrl-d should be ignored
    fn ctrl_d_non_empty() {
        let mut out = Vec::new();
        let mut history = History::new();
        let words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &words,
            &mut buf,
        )
        .unwrap();
        let mut map = TestKeyMap;
        ed.insert_str_after_cursor("not empty").unwrap();

        let res = map.handle_key(
            Key::new_mod(KeyCode::Char('d'), KeyMod::Ctrl),
            &mut ed,
            &mut EmptyCompleter,
        );
        assert_eq!(res.is_ok(), true);
    }

    #[test]
    /// ctrl-c should generate an error
    fn ctrl_c() {
        let mut out = Vec::new();
        let mut history = History::new();
        let words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &words,
            &mut buf,
        )
        .unwrap();
        let mut map = TestKeyMap;

        let res = map.handle_key(
            Key::new_mod(KeyCode::Char('c'), KeyMod::Ctrl),
            &mut ed,
            &mut EmptyCompleter,
        );
        assert_eq!(res.is_err(), true);
        assert_eq!(res.err().unwrap().kind(), ErrorKind::Interrupted);
    }
}
