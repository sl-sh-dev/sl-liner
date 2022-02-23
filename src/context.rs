use std::io;
use std::time;

use sl_console::*;

use super::*;

pub type ColorClosure = Box<dyn FnMut(&str) -> String>;

pub fn check_balanced_delimiters(buf: &Buffer) -> bool {
    let buf_vec = buf.range_graphemes_all();
    let mut stack = vec![];
    for c in buf_vec {
        match c {
            "(" | "[" | "{" => stack.push(c),
            ")" | "]" | "}" => {
                stack.pop();
            }
            _ => {}
        }
    }
    //if the stack is empty, then we should evaluate the line, as there are no unbalanced
    // delimiters.
    stack.is_empty()
}

pub fn last_non_ws_char_was_not_backslash(buf: &Buffer) -> bool {
    let mut found_backslash = false;
    for x in buf.range_graphemes_all().rev() {
        if x.trim().is_empty() || x == "\n" {
            continue;
        } else if x == "\\" {
            found_backslash = true;
            break;
        } else {
            break;
        }
    }
    // if the last non-whitespace character was not a backslash then we can evaluate the line, as
    // backslash is the user's way of indicating intent to insert a new line
    !found_backslash
}

pub struct Context {
    pub history: History,
    rules: Box<dyn EditorRules>,
    buf: String,
    handler: Box<dyn Completer>,
    keymap: Box<dyn KeyMap>,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

type WordDivider = Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>;
type LineCompleter = Box<dyn Fn(&Buffer) -> bool>;

//TODO can't have these be optional if you're also passing an optional. it's just weird.
pub struct ContextHelperBuilder {
    word_divider_fn: Option<WordDivider>,
    line_completion_fn: Option<LineCompleter>,
}

impl Default for ContextHelperBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextHelperBuilder {
    pub fn new() -> Self {
        ContextHelperBuilder {
            word_divider_fn: None,
            line_completion_fn: None,
        }
    }

    pub fn set_word_divider_fn(
        &mut self,
        word_divider_fn: Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>,
    ) -> &mut Self {
        self.word_divider_fn = Some(word_divider_fn);
        self
    }

    pub fn set_line_completion_fn(
        &mut self,
        line_completion_fn: Box<dyn Fn(&Buffer) -> bool>,
    ) -> &mut Self {
        self.line_completion_fn = Some(line_completion_fn);
        self
    }

    //TODO mention defaults
    pub fn build(self) -> ContextHelper {
        ContextHelper {
            word_divider_fn: self
                .word_divider_fn
                .unwrap_or_else(|| Box::new(get_buffer_words)),
            line_completion_fn: self
                .line_completion_fn
                .unwrap_or_else(|| Box::new(last_non_ws_char_was_not_backslash)),
        }
    }
}

pub struct ContextHelper {
    word_divider_fn: Box<dyn Fn(&Buffer) -> Vec<(usize, usize)>>,
    line_completion_fn: Box<dyn Fn(&Buffer) -> bool>,
}

impl EditorRules for ContextHelper {
    fn evaluate_on_newline(&self, buf: &Buffer) -> bool {
        (self.line_completion_fn)(buf)
    }

    fn divide_words(&self, buf: &Buffer) -> Vec<(usize, usize)> {
        (self.word_divider_fn)(buf)
    }
}

impl Context {
    pub fn new() -> Self {
        Context {
            history: History::new(),
            rules: Box::new(ContextHelperBuilder::default().build()),
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

    pub fn set_word_divider(&mut self, helper: Box<dyn EditorRules>) -> &mut Self {
        self.rules = helper;
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
    /// use sl_liner::{Context, Completer, Prompt};
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
        con_init()?;
        let mut conout = conout().lock().into_raw_mode()?;
        let mut conin = conin();
        let mut ed = Editor::new_with_init_buffer(
            &mut conout,
            prompt,
            f,
            &mut self.history,
            &mut self.buf,
            buffer,
            Some(&*self.rules),
        )?;
        self.keymap.init(&mut ed);
        ed.use_closure(false);
        let mut do_color = false;
        let timeout = time::Duration::from_millis(200);
        loop {
            let c = if do_color {
                conin.get_event_timeout(timeout)
            } else {
                conin.get_event()
            };
            match c {
                Some(Ok(sl_console::event::Event::Key(key))) => {
                    do_color = true;
                    if self.keymap.handle_key(key, &mut ed, &mut *self.handler)? {
                        break;
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(err)) if err.kind() == io::ErrorKind::WouldBlock => {
                    if do_color {
                        ed.use_closure(true);
                        ed.display_term()?;
                        ed.use_closure(false);
                        do_color = false;
                    }
                }
                Some(Err(err)) => {
                    return Err(err);
                }
                None => {}
            }
        }
        Ok(ed.into())
    }
}
