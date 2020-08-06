use std::fs::OpenOptions;
use std::io::{self, stdin, stdout};
use std::os::unix::fs::OpenOptionsExt;

use termion::input::TermRead;
use termion::raw::IntoRawMode;

use super::*;
use crate::editor::Prompt;

pub type ColorClosure = Box<dyn FnMut(&str) -> String>;

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
    word_divider_fn: Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>,
    buf: String,
    handler: Box<dyn Completer>,
    keymap: Box<dyn KeyMap>,
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
            handler: Box::new(EmptyCompleter::new()),
            keymap: Box::new(keymap::Emacs::new()),
        }
    }

    pub fn set_completer(&mut self, completer: Box<dyn Completer>) -> &mut Self {
        self.handler = completer;
        self
    }

    pub fn set_keymap(&mut self, keymap: Box<dyn KeyMap>) -> &mut Self {
        self.keymap = keymap;
        self
    }

    pub fn set_word_divider(
        &mut self,
        word_divider_fn: Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>,
    ) -> &mut Self {
        self.word_divider_fn = word_divider_fn;
        self
    }

    /// Creates an `Editor` and feeds it keypresses from stdin until the line is entered.
    /// The output is stdout.
    /// The returned line has the newline removed.
    /// Before returning, will revert all changes to the history buffers.
    pub fn read_line(&mut self, prompt: Prompt, f: Option<ColorClosure>) -> io::Result<String> {
        self.edit_line(prompt, f, Buffer::new())
    }

    /// Same as `Context.read_line()`, but passes the provided initial buffer to the editor.
    ///
    /// ```no_run
    /// use liner::{Context, Completer, Prompt};
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
    /// context.set_completer(Box::new(EmptyCompleter{}));
    /// let line =
    ///     context.edit_line(Prompt::from("[prompt]$ "),
    ///                       Some(Box::new(|s| String::from(s))),
    ///                       "some initial buffer");
    /// ```
    pub fn edit_line<B: Into<Buffer>>(
        &mut self,
        prompt: Prompt,
        f: Option<ColorClosure>,
        buffer: B,
    ) -> io::Result<String> {
        let stdout = stdout();
        let mut stdout = stdout.lock().into_raw_mode()?;
        let mut ed = Editor::new_with_init_buffer(
            &mut stdout,
            prompt,
            f,
            &mut self.history,
            &self.word_divider_fn,
            &mut self.buf,
            buffer,
        )?;
        self.keymap.init(&mut ed);

        if termion::is_tty(&stdin()) {
            let tty = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open("/dev/tty")
                .unwrap();
            let sleep_millis = std::time::Duration::from_millis(10);
            ed.use_closure(false);
            let mut displayed = false;
            let mut sleep_ms = 0;
            for c in tty.keys() {
                if let Ok(key) = c {
                    if self.keymap.handle_key(key, &mut ed, &mut *self.handler)? {
                        break;
                    }
                    displayed = false;
                    sleep_ms = 0;
                } else {
                    if !displayed && sleep_ms > 250 {
                        ed.use_closure(true);
                        ed.display()?;
                        ed.use_closure(false);
                        displayed = true;
                    }
                    std::thread::sleep(sleep_millis);
                    sleep_ms += 10;
                }
            }
        } else {
            // No tty, so don't bother with the color closure.
            ed.use_closure(false);
            for c in stdin().lock().keys() {
                if self
                    .keymap
                    .handle_key(c.unwrap(), &mut ed, &mut *self.handler)?
                {
                    break;
                }
            }
        }

        Ok(ed.into())
    }
}
