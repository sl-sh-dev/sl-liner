use sl_console::event::{Key, KeyCode, KeyMod};
use std::io;

use crate::buffer::Buffer;
use crate::CursorPosition;
use crate::Editor;
use crate::KeyMap;

/// Emacs keybindings for `Editor`. This is the default for `Context::read_line()`.
///
/// ```
/// use sl_liner::*;
/// use sl_liner::keymap;
///
/// struct EmptyCompleter;
/// impl Completer for EmptyCompleter {
///     fn completions(&mut self, _start: &str) -> Vec<String> {
///         Vec::new()
///     }
/// }
///
/// let mut context = Context::new();
/// // This will hang github actions on windows...
/// //let res = context.read_line(Prompt::from("[prompt]$ "), None);
/// ```
#[derive(Default, Clone)]
pub struct Emacs {
    last_arg_fetch_index: Option<usize>,
}

impl Emacs {
    pub fn new() -> Self {
        Self::default()
    }

    fn handle_ctrl_key<'a>(&mut self, c: char, ed: &mut Editor<'a>) -> io::Result<()> {
        match c {
            'l' => ed.clear(),
            'a' => ed.move_cursor_to_start_of_line(),
            'e' => ed.move_cursor_to_end_of_line(),
            'b' => ed.move_cursor_left(1),
            'f' => ed.move_cursor_right(1),
            'd' => ed.delete_after_cursor(),
            'p' => ed.move_up(),
            'n' => ed.move_down(),
            'u' => ed.delete_all_before_cursor(),
            'k' => ed.delete_all_after_cursor(),
            'w' => ed.delete_word_before_cursor(true),
            'x' => {
                if ed.undo().is_some() {
                    ed.move_cursor_to_end_of_line()
                } else {
                    ed.display()
                }
            }
            _ => Ok(()),
        }
    }

    fn handle_alt_key<'a>(&mut self, c: char, ed: &mut Editor<'a>) -> io::Result<()> {
        match c {
            '<' => ed.move_to_start_of_history(),
            '>' => ed.move_to_end_of_history(),
            '\x7F' => ed.delete_word_before_cursor(true),
            'f' => emacs_move_word(ed, EmacsMoveDir::Right),
            'b' => emacs_move_word(ed, EmacsMoveDir::Left),
            'r' => {
                ed.revert()?;
                Ok(())
            }
            '.' => self.handle_last_arg_fetch(ed),
            _ => Ok(()),
        }
    }

    fn handle_last_arg_fetch<'a>(&mut self, ed: &mut Editor<'a>) -> io::Result<()> {
        // Empty history means no last arg to fetch.
        if ed.history().is_empty() {
            return Ok(());
        }

        let history_index = match self.last_arg_fetch_index {
            Some(0) => return Ok(()),
            Some(x) => x - 1,
            None => ed
                .current_history_location()
                .unwrap_or(ed.history().len() - 1),
        };

        // If did a last arg fetch just before this, we need to delete it so it can be replaced by
        // this last arg fetch.
        if self.last_arg_fetch_index.is_some() {
            let buffer_len = ed.current_buffer().num_chars();
            if let Some(last_arg_len) = ed.current_buffer().last_arg().map(|x| x.len()) {
                ed.delete_until(buffer_len - last_arg_len)?;
            }
        }

        // Actually insert it
        let buf: Buffer = ed.history()[history_index].into();
        if let Some(last_arg) = buf.last_arg() {
            ed.insert_chars_after_cursor(last_arg)?;
        }

        // Edit the index in case the user does a last arg fetch again.
        self.last_arg_fetch_index = Some(history_index);

        Ok(())
    }
}

impl KeyMap for Emacs {
    fn init<'a>(&mut self, _ed: &mut Editor<'a>) {
        self.last_arg_fetch_index = None;
    }

    fn handle_key_core<'a>(&mut self, key: Key, ed: &mut Editor<'a>) -> io::Result<()> {
        match (key.code, key.mods) {
            (KeyCode::Char('.'), Some(KeyMod::Alt)) => {}
            _ => self.last_arg_fetch_index = None,
        }

        match (key.code, key.mods) {
            (KeyCode::Char(c), key_mod) => match key_mod {
                None => ed.insert_after_cursor(c),
                Some(KeyMod::Alt) => self.handle_alt_key(c, ed),
                Some(KeyMod::Ctrl) => self.handle_ctrl_key(c, ed),
                _ => Ok(()),
            },
            (key_code, None) => match key_code {
                KeyCode::Left => ed.move_cursor_left(1),
                KeyCode::Right => ed.move_cursor_right(1),
                KeyCode::Up => ed.move_up(),
                KeyCode::Down => ed.move_down(),
                KeyCode::Home => ed.move_cursor_to_start_of_line(),
                KeyCode::End => ed.move_cursor_to_end_of_line(),
                KeyCode::Backspace => ed.delete_before_cursor(),
                KeyCode::Delete => ed.delete_after_cursor(),
                KeyCode::Null => Ok(()),
                _ => Ok(()),
            },
            _ => Ok(()),
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum EmacsMoveDir {
    Left,
    Right,
}

fn emacs_move_word(ed: &mut Editor, direction: EmacsMoveDir) -> io::Result<()> {
    let (words, pos) = ed.get_words_and_cursor_position();

    let word_index = match pos {
        CursorPosition::InWord(i) => Some(i),
        CursorPosition::OnWordLeftEdge(mut i) => {
            if i > 0 && direction == EmacsMoveDir::Left {
                i -= 1;
            }
            Some(i)
        }
        CursorPosition::OnWordRightEdge(mut i) => {
            if i < words.len() - 1 && direction == EmacsMoveDir::Right {
                i += 1;
            }
            Some(i)
        }
        CursorPosition::InSpace(left, right) => match direction {
            EmacsMoveDir::Left => left,
            EmacsMoveDir::Right => right,
        },
    };

    match word_index {
        None => Ok(()),
        Some(i) => {
            let (start, end) = words[i];

            let new_cursor_pos = match direction {
                EmacsMoveDir::Left => start,
                EmacsMoveDir::Right => end,
            };

            ed.move_cursor_to(new_cursor_pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::get_buffer_words;
    use crate::editor::Prompt;
    use crate::{Completer, Editor, History, KeyMap};
    use sl_console::event::Key;

    fn simulate_key_codes<'a, 'b, M: KeyMap, I>(
        keymap: &mut M,
        ed: &mut Editor<'a>,
        keys: I,
    ) -> bool
    where
        I: IntoIterator<Item = &'b KeyCode>,
    {
        for k in keys {
            if keymap
                .handle_key(
                    Key {
                        code: *k,
                        mods: None,
                    },
                    ed,
                    &mut EmptyCompleter,
                )
                .unwrap()
            {
                return true;
            }
        }

        false
    }

    fn simulate_keys<'a, 'b, M: KeyMap, I>(keymap: &mut M, ed: &mut Editor<'a>, keys: I) -> bool
    where
        I: IntoIterator<Item = &'b Key>,
    {
        for k in keys {
            if keymap.handle_key(*k, ed, &mut EmptyCompleter).unwrap() {
                return true;
            }
        }

        false
    }

    struct EmptyCompleter;

    impl Completer for EmptyCompleter {
        fn completions(&mut self, _start: &str) -> Vec<String> {
            Vec::default()
        }
    }

    #[test]
    fn enter_is_done() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut words,
            &mut buf,
        )
        .unwrap();
        let mut map = Emacs::new();
        ed.insert_str_after_cursor("done").unwrap();
        assert_eq!(ed.cursor(), 4);

        assert!(simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Char('\n')].iter()
        ));

        assert_eq!(ed.cursor(), 4);
        assert_eq!(String::from(ed), "done");
    }

    #[test]
    fn move_cursor_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut words,
            &mut buf,
        )
        .unwrap();
        let mut map = Emacs::new();
        ed.insert_str_after_cursor("let").unwrap();
        assert_eq!(ed.cursor(), 3);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Left, KeyCode::Char('f')].iter(),
        );

        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "left");
    }

    #[test]
    fn move_word() {
        let mut out = Vec::new();

        let mut history = History::new();
        let mut words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut words,
            &mut buf,
        )
        .unwrap();
        let mut map = Emacs::new();
        ed.insert_str_after_cursor("abc def ghi").unwrap();
        assert_eq!(ed.cursor(), 11);

        simulate_keys(
            &mut map,
            &mut ed,
            [Key::new_mod(KeyCode::Char('b'), KeyMod::Alt)].iter(),
        );

        // Move to `g`
        assert_eq!(ed.cursor(), 8);

        simulate_keys(
            &mut map,
            &mut ed,
            [
                Key::new_mod(KeyCode::Char('b'), KeyMod::Alt),
                Key::new_mod(KeyCode::Char('f'), KeyMod::Alt),
            ]
            .iter(),
        );

        // Move to the char after `f`
        assert_eq!(ed.cursor(), 7);
    }

    #[test]
    fn cursor_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut words,
            &mut buf,
        )
        .unwrap();
        let mut map = Emacs::new();
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        simulate_key_codes(
            &mut map,
            &mut ed,
            [KeyCode::Left, KeyCode::Left, KeyCode::Right].iter(),
        );

        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    /// ctrl-h should act as backspace
    fn ctrl_h() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut words = Box::new(get_buffer_words);
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut words,
            &mut buf,
        )
        .unwrap();
        let mut map = Emacs::new();
        ed.insert_str_after_cursor("not empty").unwrap();

        let res = map.handle_key(
            Key::new_mod(KeyCode::Char('h'), KeyMod::Ctrl),
            &mut ed,
            &mut EmptyCompleter,
        );
        assert_eq!(res.is_ok(), true);
        assert_eq!(ed.current_buffer().to_string(), "not empt".to_string());
    }
}
