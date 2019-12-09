use std::io::{self, stdin, stdout};
use termion::input::TermRead;
use termion::raw::IntoRawMode;

use super::*;
use keymap;

pub type ColorClosure = Box<dyn Fn(&str) -> String>;

/// The default for `Context.word_divider_fn`.
pub fn get_buffer_words(buf: &Buffer) -> Vec<(usize, usize)> {
    let mut res = Vec::new();

    let mut word_start = None;
    let mut just_had_backslash = false;

    for (i, &c) in buf.chars().enumerate() {
        if c == '\\' {
            just_had_backslash = true;
            continue;
        }

        if let Some(start) = word_start {
            if c == ' ' && !just_had_backslash {
                res.push((start, i));
                word_start = None;
            }
        } else if c != ' ' {
            word_start = Some(i);
        }

        just_had_backslash = false;
    }

    if let Some(start) = word_start {
        res.push((start, buf.num_chars()));
    }

    res
}

pub struct Context {
    pub history: History,
    pub word_divider_fn: Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>,
    pub buf: String,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    pub fn new() -> Self {
        Context {
            history: History::new(),
            word_divider_fn: Box::new(get_buffer_words),
            buf: String::with_capacity(512),
        }
    }

    /// Creates an `Editor` and feeds it keypresses from stdin until the line is entered.
    /// The output is stdout.
    /// The returned line has the newline removed.
    /// Before returning, will revert all changes to the history buffers.
    pub fn read_line<P: Into<String>>(
        &mut self,
        prompt: P,
        f: Option<ColorClosure>,
        handler: &mut dyn Completer,
        keymap: Option<&mut dyn KeyMap>,
    ) -> io::Result<String> {
        self.read_line_with_init_buffer(prompt, handler, f, Buffer::new(), keymap)
    }

    /// Same as `Context.read_line()`, but passes the provided initial buffer to the editor.
    ///
    /// ```no_run
    /// use liner::{Context, Completer};
    ///
    /// struct EmptyCompleter;
    ///
    /// impl Completer for EmptyCompleter {
    ///     fn completions(&mut self, _start: &str) -> Vec<String> {
    ///         Vec::new()
    ///     }
    /// }
    ///
    /// let mut context = Context::new();
    /// let line =
    ///     context.read_line_with_init_buffer("[prompt]$ ",
    ///                                        &mut EmptyCompleter,
    ///                                        Some(Box::new(|s| String::from(s))),
    ///                                        "some initial buffer",
    ///                                        None);
    /// ```
    pub fn read_line_with_init_buffer<P: Into<String>, B: Into<Buffer>>(
        &mut self,
        prompt: P,
        handler: &mut dyn Completer,
        f: Option<ColorClosure>,
        buffer: B,
        keymap: Option<&mut dyn KeyMap>,
    ) -> io::Result<String> {
        let mut stdout = stdout().into_raw_mode()?;
        let ed = Editor::new_with_init_buffer(&mut stdout, prompt, f, self, buffer)?;
        let mut km;
        let keymap = match keymap {
            Some(keymap) => keymap,
            None => {
                km = keymap::Emacs::new();
                &mut km
            }
        };
        Self::handle_keys(keymap, ed, handler)
    }

    fn handle_keys<'a>(
        keymap: &mut dyn KeyMap,
        mut ed: Editor<'a>,
        handler: &mut dyn Completer,
    ) -> io::Result<String> {
        keymap.init(&mut ed);
        for c in stdin().keys() {
            if keymap.handle_key(c.unwrap(), &mut ed, handler)? {
                break;
            }
        }

        Ok(ed.into())
    }
}
