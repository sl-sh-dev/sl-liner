use std::io::{self, stdin, stdout, Stdout, Write};
use termion::input::TermRead;
use termion::raw::{IntoRawMode, RawTerminal};

use super::*;
use keymap;

pub type ColorClosure = Box<Fn(&str) -> String>;

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

/// The key bindings to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyBindings {
    Vi,
    Emacs,
}

pub struct Context {
    pub history: History,
    pub word_divider_fn: Box<Fn(&Buffer) -> Vec<(usize, usize)>>,
    pub key_bindings: KeyBindings,
}

impl Context {
    pub fn new() -> Self {
        Context {
            history: History::new(),
            word_divider_fn: Box::new(get_buffer_words),
            key_bindings: KeyBindings::Emacs,
        }
    }

    /// Creates an `Editor` and feeds it keypresses from stdin until the line is entered.
    /// The output is stdout.
    /// The returned line has the newline removed.
    /// Before returning, will revert all changes to the history buffers.
    pub fn read_line<P: Into<String>, C: Completer<RawTerminal<Stdout>>>(
        &mut self,
        prompt: P,
        f: Option<ColorClosure>,
        handler: &mut C,
    ) -> io::Result<String> {
        self.read_line_with_init_buffer(prompt, handler, f, Buffer::new())
    }

    /// Same as `Context.read_line()`, but passes the provided initial buffer to the editor.
    ///
    /// ```no_run
    /// use liner::{Context, Completer};
    ///
    /// struct EmptyCompleter;
    ///
    /// impl<W: std::io::Write> Completer<W> for EmptyCompleter {
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
    ///                                        "some initial buffer");
    /// ```
    pub fn read_line_with_init_buffer<
        P: Into<String>,
        B: Into<Buffer>,
        C: Completer<RawTerminal<Stdout>>,
    >(
        &mut self,
        prompt: P,
        handler: &mut C,
        f: Option<ColorClosure>,
        buffer: B,
    ) -> io::Result<String> {
        let res = {
            let mut stdout = stdout().into_raw_mode()?;
            let mut ed = Editor::new_with_init_buffer(stdout, prompt, f, self, buffer)?;
            match self.key_bindings {
                KeyBindings::Emacs => Self::handle_keys(keymap::Emacs::new(), ed, handler),
                KeyBindings::Vi => Self::handle_keys(keymap::Vi::new(&mut ed), ed, handler),
            }
        };

        //self.revert_all_history();
        res
    }

    fn handle_keys<'a, W: Write, M: KeyMap, C: Completer<W>>(
        mut keymap: M,
        mut ed: Editor<'a, W>,
        handler: &mut C,
    ) -> io::Result<String> {
        for c in stdin().keys() {
            if keymap.handle_key(c.unwrap(), &mut ed, handler)? {
                break;
            }
        }

        Ok(ed.into())
    }

    pub fn revert_all_history(&mut self) {
        for buf in &mut self.history.buffers {
            buf.revert();
        }
    }
}
