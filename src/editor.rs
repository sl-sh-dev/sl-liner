use sl_console::{self, clear, color, cursor};
use std::cmp;
use std::cmp::Ordering;
use std::fmt::{self, Write};
use std::io;

use super::complete::Completer;
use crate::context::ColorClosure;
use crate::event::*;
use crate::util;
use crate::Buffer;
use crate::History;

/// User-defined prompt.
///
/// # Examples
///
/// You simply define a static prompt that holds a string.
/// The prefix and suffix fields are intended for keybinds to change the
/// prompt (ie the mode in vi).
/// ```
/// # use sl_liner::Prompt;
/// let prompt = Prompt::from("prompt$ ");
/// assert_eq!(&prompt.to_string(), "prompt$ ");
/// ```
pub struct Prompt {
    pub prefix: Option<String>,
    pub prompt: String,
    pub suffix: Option<String>,
}

impl Prompt {
    /// Constructs a static prompt.
    pub fn from<P: Into<String>>(prompt: P) -> Self {
        Prompt {
            prefix: None,
            prompt: prompt.into(),
            suffix: None,
        }
    }

    pub fn prefix(&self) -> &str {
        match &self.prefix {
            Some(prefix) => prefix,
            None => "",
        }
    }

    pub fn suffix(&self) -> &str {
        match &self.suffix {
            Some(suffix) => suffix,
            None => "",
        }
    }
}

impl fmt::Display for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.prefix(), self.prompt, self.suffix())
    }
}

/// Represents the position of the cursor relative to words in the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPosition {
    /// The cursor is in the word with the specified index.
    InWord(usize),

    /// The cursor is on the left edge of the word with the specified index.
    /// For example: `abc |hi`, where `|` is the cursor.
    OnWordLeftEdge(usize),

    /// The cursor is on the right edge of the word with the specified index.
    /// For example: `abc| hi`, where `|` is the cursor.
    OnWordRightEdge(usize),

    /// The cursor is not in contact with any word. Each `Option<usize>` specifies the index of the
    /// closest word to the left and right, respectively, or `None` if there is no word on that side.
    InSpace(Option<usize>, Option<usize>),
}

impl CursorPosition {
    pub fn get(cursor: usize, words: &[(usize, usize)]) -> CursorPosition {
        use CursorPosition::*;

        if words.is_empty() {
            return InSpace(None, None);
        } else if cursor == words[0].0 {
            return OnWordLeftEdge(0);
        } else if cursor < words[0].0 {
            return InSpace(None, Some(0));
        }

        for (i, &(start, end)) in words.iter().enumerate() {
            if start == cursor {
                return OnWordLeftEdge(i);
            } else if end == cursor {
                return OnWordRightEdge(i);
            } else if start < cursor && cursor < end {
                return InWord(i);
            } else if cursor < start {
                return InSpace(Some(i - 1), Some(i));
            }
        }

        InSpace(Some(words.len() - 1), None)
    }
}

/// The core line editor. Displays and provides editing for history and the new buffer.
pub struct Editor<'a> {
    prompt: Prompt,
    out: &'a mut dyn io::Write,
    history: &'a mut History,
    word_divider_fn: &'a dyn Fn(&Buffer) -> Vec<(usize, usize)>,
    buf: &'a mut String,

    // A closure that is evaluated just before we write to out.
    // This allows us to do custom syntax highlighting and other fun stuff.
    closure: Option<ColorClosure>,
    // Use the closure if it is set.
    use_closure: bool,
    // Last string that was colorized and last colorized version.
    color_lines: Option<(String, String)>,

    // The location of the cursor. Note that the cursor does not lie on a char, but between chars.
    // So, if `cursor == 0` then the cursor is before the first char,
    // and if `cursor == 1` ten the cursor is after the first char and before the second char.
    cursor: usize,

    // Buffer for the new line (ie. not from editing history)
    new_buf: Buffer,

    // Buffer to use when editing history so we do not overwrite it.
    hist_buf: Buffer,
    hist_buf_valid: bool,

    // None if we're on the new buffer, else the index of history
    cur_history_loc: Option<usize>,

    // The line of the cursor relative to the prompt. 1-indexed.
    // So if the cursor is on the same line as the prompt, `term_cursor_line == 1`.
    // If the cursor is on the line below the prompt, `term_cursor_line == 2`.
    term_cursor_line: usize,

    // The next completion to suggest, or none
    show_completions_hint: Option<(Vec<String>, Option<usize>)>,

    // Show autosuggestions based on history
    show_autosuggestions: bool,

    // if set, the cursor will not be allow to move one past the end of the line, this is necessary
    // for Vi's normal mode.
    pub no_eol: bool,

    reverse_search: bool,
    forward_search: bool,
    buffer_changed: bool,

    history_subset_index: Vec<usize>,
    history_subset_loc: Option<usize>,

    autosuggestion: Option<Buffer>,

    history_fresh: bool,
}

macro_rules! cur_buf_mut {
    ($s:expr) => {{
        $s.buffer_changed = true;
        match $s.cur_history_loc {
            Some(i) => {
                if !$s.hist_buf_valid {
                    $s.hist_buf.copy_buffer(&$s.history[i].into());
                    $s.hist_buf_valid = true;
                }
                &mut $s.hist_buf
            }
            _ => &mut $s.new_buf,
        }
    }};
}

macro_rules! cur_buf {
    ($s:expr) => {
        match $s.cur_history_loc {
            Some(_) if $s.hist_buf_valid => &$s.hist_buf,
            //Some(i) => &$s.history[i],
            _ => &$s.new_buf,
        }
    };
}

fn fmt_io_err(err: std::fmt::Error) -> io::Error {
    let msg = format!("{}", err);
    io::Error::new(io::ErrorKind::Other, msg)
}

impl<'a> Editor<'a> {
    pub fn new(
        out: &'a mut dyn io::Write,
        prompt: Prompt,
        f: Option<ColorClosure>,
        history: &'a mut History,
        word_divider_fn: &'a dyn Fn(&Buffer) -> Vec<(usize, usize)>,
        buf: &'a mut String,
    ) -> io::Result<Self> {
        Editor::new_with_init_buffer(out, prompt, f, history, word_divider_fn, buf, Buffer::new())
    }

    pub fn new_with_init_buffer<B: Into<Buffer>>(
        out: &'a mut dyn io::Write,
        prompt: Prompt,
        f: Option<ColorClosure>,
        history: &'a mut History,
        word_divider_fn: &'a dyn Fn(&Buffer) -> Vec<(usize, usize)>,
        buf: &'a mut String,
        buffer: B,
    ) -> io::Result<Self> {
        out.write_all("⏎".as_bytes())?;
        for _ in 0..(util::terminal_width().unwrap_or(80) - 1) {
            out.write_all(b" ")?; // if the line is not empty, overflow on next line
        }
        out.write_all("\r \r".as_bytes())?; // Erase the "⏎" if nothing overwrites it
        let Prompt {
            prefix,
            mut prompt,
            suffix,
        } = prompt;
        for (i, pline) in prompt.split('\n').enumerate() {
            if i > 0 {
                out.write_all(b"\r\n")?;
            }
            out.write_all(pline.as_bytes())?;
        }
        if let Some(index) = prompt.rfind('\n') {
            prompt = prompt.split_at(index + 1).1.into()
        }
        let prompt = Prompt {
            prefix,
            prompt,
            suffix,
        };
        let mut ed = Editor {
            prompt,
            cursor: 0,
            out,
            closure: f,
            use_closure: true,
            color_lines: None,
            new_buf: buffer.into(),
            hist_buf: Buffer::new(),
            hist_buf_valid: false,
            cur_history_loc: None,
            history,
            word_divider_fn,
            buf,
            show_completions_hint: None,
            show_autosuggestions: true,
            term_cursor_line: 1,
            no_eol: false,
            reverse_search: false,
            forward_search: false,
            buffer_changed: false,
            history_subset_index: vec![],
            history_subset_loc: None,
            autosuggestion: None,
            history_fresh: false,
        };

        if !ed.new_buf.is_empty() {
            ed.move_cursor_to_end_of_line()?;
        }
        ed.display()?;
        Ok(ed)
    }

    pub fn set_closure(&mut self, closure: ColorClosure) -> &mut Self {
        self.closure = Some(closure);
        self
    }

    pub fn use_closure(&mut self, use_closure: bool) {
        self.use_closure = use_closure;
    }

    fn is_search(&self) -> bool {
        self.reverse_search || self.forward_search
    }

    fn clear_search(&mut self) {
        self.reverse_search = false;
        self.forward_search = false;
        self.history_subset_loc = None;
        self.history_subset_index.clear();
    }

    /// None if we're on the new buffer, else the index of history
    pub fn current_history_location(&self) -> Option<usize> {
        self.cur_history_loc
    }

    pub fn get_words_and_cursor_position(&self) -> (Vec<(usize, usize)>, CursorPosition) {
        let word_fn = &self.word_divider_fn;
        let words = word_fn(cur_buf!(self));
        let pos = CursorPosition::get(self.cursor, &words);
        (words, pos)
    }

    pub fn history(&mut self) -> &mut History {
        self.history
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    // XXX: Returning a bool to indicate doneness is a bit awkward, maybe change it
    pub fn handle_newline(&mut self) -> io::Result<bool> {
        self.history_fresh = false;
        if self.is_search() {
            self.accept_autosuggestion()?;
        }
        self.clear_search();
        if self.show_completions_hint.is_some() {
            self.show_completions_hint = None;
            return Ok(false);
        }

        let last_char = cur_buf!(self).last();
        if last_char == Some(&'\\') {
            let buf = cur_buf_mut!(self);
            buf.push('\n');
            self.cursor = buf.num_chars();
            self.display()?;
            Ok(false)
        } else {
            self.cursor = cur_buf!(self).num_chars();
            self._display(false)?;
            self.out.write_all(b"\r\n")?;
            self.show_completions_hint = None;
            Ok(true)
        }
    }

    fn search_history_loc(&self) -> Option<usize> {
        self.history_subset_loc
            .and_then(|i| self.history_subset_index.get(i).cloned())
    }

    fn freshen_history(&mut self) {
        if !self.history_fresh && self.history.load_history(false).is_ok() {
            self.history_fresh = true;
        }
    }

    /// Refresh incremental search, either when started or when the buffer changes.
    fn refresh_search(&mut self, forward: bool) {
        let search_history_loc = self.search_history_loc();
        self.history_subset_index = self.history.search_index(&self.new_buf.to_string());
        if !self.history_subset_index.is_empty() {
            self.history_subset_loc = if forward {
                Some(0)
            } else {
                Some(self.history_subset_index.len() - 1)
            };
            if let Some(target_loc) = search_history_loc {
                for (i, history_loc) in self.history_subset_index.iter().enumerate() {
                    if target_loc <= *history_loc {
                        if forward || target_loc == *history_loc || i == 0 {
                            self.history_subset_loc = Some(i);
                        } else {
                            self.history_subset_loc = Some(i - 1);
                        }
                        break;
                    }
                }
            }
        } else {
            self.history_subset_loc = None;
        }

        self.reverse_search = !forward;
        self.forward_search = forward;
        self.cur_history_loc = None;
        self.hist_buf_valid = false;
        self.buffer_changed = false;
    }

    /// Begin or continue a search through history.  If forward is true then start at top (or
    /// current_history_loc if set). If started with forward true then incremental search goes
    /// forward (top to bottom) other wise reverse (bottom to top).  It is valid to continue a
    /// search with forward changed (i.e. reverse search direction for one result).
    pub fn search(&mut self, forward: bool) -> io::Result<()> {
        if !self.is_search() {
            self.freshen_history();
            self.refresh_search(forward);
        } else if !self.history_subset_index.is_empty() {
            self.history_subset_loc = if let Some(p) = self.history_subset_loc {
                if forward {
                    if p < self.history_subset_index.len() - 1 {
                        Some(p + 1)
                    } else {
                        Some(0)
                    }
                } else if p > 0 {
                    Some(p - 1)
                } else {
                    Some(self.history_subset_index.len() - 1)
                }
            } else {
                None
            };
        }
        self.display()?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Attempts to undo an action on the current buffer.
    ///
    /// Returns `Ok(true)` if an action was undone.
    /// Returns `Ok(false)` if there was no action to undo.
    pub fn undo(&mut self) -> io::Result<bool> {
        let did = cur_buf_mut!(self).undo();
        if did {
            self.move_cursor_to_end_of_line()?;
        } else {
            self.display()?;
        }
        Ok(did)
    }

    pub fn redo(&mut self) -> io::Result<bool> {
        let did = cur_buf_mut!(self).redo();
        if did {
            self.move_cursor_to_end_of_line()?;
        } else {
            self.display()?;
        }
        Ok(did)
    }

    pub fn revert(&mut self) -> io::Result<bool> {
        let did = cur_buf_mut!(self).revert();
        if did {
            self.move_cursor_to_end_of_line()?;
        } else {
            self.display()?;
        }
        Ok(did)
    }

    fn print_completion_list(
        completions: &[String],
        highlighted: Option<usize>,
        output_buf: &mut String,
    ) -> io::Result<usize> {
        use std::cmp::max;

        let (w, _) = sl_console::terminal_size()?;

        // XXX wide character support
        let max_word_size = completions.iter().fold(1, |m, x| max(m, x.chars().count()));
        let cols = max(1, w as usize / (max_word_size));
        let col_width = 2 + w as usize / cols;
        let cols = max(1, w as usize / col_width);

        let lines = completions.len() / cols;

        let mut i = 0;
        for (index, com) in completions.iter().enumerate() {
            match i.cmp(&cols) {
                Ordering::Greater => unreachable!(),
                Ordering::Less => {}
                Ordering::Equal => {
                    output_buf.push_str("\r\n");
                    i = 0;
                }
            }

            if Some(index) == highlighted {
                write!(
                    output_buf,
                    "{}{}",
                    color::Black.fg_str(),
                    color::White.bg_str()
                )
                .map_err(fmt_io_err)?;
            }
            write!(output_buf, "{:<1$}", com, col_width).map_err(fmt_io_err)?;
            if Some(index) == highlighted {
                write!(
                    output_buf,
                    "{}{}",
                    color::Reset.bg_str(),
                    color::Reset.fg_str()
                )
                .map_err(fmt_io_err)?;
            }

            i += 1;
        }

        Ok(lines)
    }

    pub fn skip_completions_hint(&mut self) {
        self.show_completions_hint = None;
    }

    pub fn complete(&mut self, handler: &mut dyn Completer) -> io::Result<()> {
        handler.on_event(Event::new(self, EventKind::BeforeComplete));

        if let Some((completions, i_in)) = self.show_completions_hint.take() {
            let i = i_in.map_or(0, |i| (i + 1) % completions.len());

            match i_in {
                Some(x) if cur_buf!(self) == &Buffer::from(&completions[x][..]) => {
                    cur_buf_mut!(self).truncate(0);
                    self.cursor = 0;
                }
                _ => self.delete_word_before_cursor(false)?,
            }
            self.insert_str_after_cursor(&completions[i])?;

            self.show_completions_hint = Some((completions, Some(i)));
        }
        if self.show_completions_hint.is_some() {
            self.display()?;
            return Ok(());
        }

        let (word, completions) = {
            let word_range = self.get_word_before_cursor(false);
            let buf = cur_buf_mut!(self);

            let word = match word_range {
                Some((start, end)) => buf.range(start, end),
                None => "".into(),
            };

            let mut completions = handler.completions(word.as_ref());
            completions.sort();
            completions.dedup();
            (word, completions)
        };

        if completions.is_empty() {
            // Do nothing.
            self.show_completions_hint = None;
            Ok(())
        } else if completions.len() == 1 {
            self.show_completions_hint = None;
            self.delete_word_before_cursor(false)?;
            self.insert_str_after_cursor(completions[0].as_ref())
        } else {
            let common_prefix = util::find_longest_common_prefix(
                &completions
                    .iter()
                    .map(|x| x.chars().collect())
                    .collect::<Vec<Vec<char>>>()[..],
            );

            if let Some(p) = common_prefix {
                let s = p.iter().cloned().collect::<String>();

                if s.len() > word.len() && s.starts_with(&word[..]) {
                    self.delete_word_before_cursor(false)?;
                    return self.insert_str_after_cursor(s.as_ref());
                }
            }

            self.show_completions_hint = Some((completions, None));
            self.display()?;

            Ok(())
        }
    }

    fn get_word_before_cursor(&self, ignore_space_before_cursor: bool) -> Option<(usize, usize)> {
        let (words, pos) = self.get_words_and_cursor_position();
        match pos {
            CursorPosition::InWord(i) => Some(words[i]),
            CursorPosition::InSpace(Some(i), _) => {
                if ignore_space_before_cursor {
                    Some(words[i])
                } else {
                    None
                }
            }
            CursorPosition::InSpace(None, _) => None,
            CursorPosition::OnWordLeftEdge(i) => {
                if ignore_space_before_cursor && i > 0 {
                    Some(words[i - 1])
                } else {
                    None
                }
            }
            CursorPosition::OnWordRightEdge(i) => Some(words[i]),
        }
    }

    /// Deletes the word preceding the cursor.
    /// If `ignore_space_before_cursor` is true and there is space directly before the cursor,
    /// this method ignores that space until it finds a word.
    /// If `ignore_space_before_cursor` is false and there is space directly before the cursor,
    /// nothing is deleted.
    pub fn delete_word_before_cursor(
        &mut self,
        ignore_space_before_cursor: bool,
    ) -> io::Result<()> {
        if let Some((start, _)) = self.get_word_before_cursor(ignore_space_before_cursor) {
            let moved = cur_buf_mut!(self).remove(start, self.cursor);
            self.cursor -= moved;
        }
        self.display()
    }

    /// Clears the screen then prints the prompt and current buffer.
    pub fn clear(&mut self) -> io::Result<()> {
        write!(&mut self.buf, "{}{}", clear::All, cursor::Goto(1, 1)).map_err(fmt_io_err)?;

        self.term_cursor_line = 1;
        self.clear_search();
        self.display()
    }

    /// Move up (backwards) in history.
    pub fn move_up(&mut self) -> io::Result<()> {
        if self.is_search() {
            self.search(false)
        } else {
            self.hist_buf_valid = false;
            self.freshen_history();
            if self.new_buf.num_chars() > 0 {
                match self.history_subset_loc {
                    Some(i) if i > 0 => {
                        self.history_subset_loc = Some(i - 1);
                        self.cur_history_loc = Some(self.history_subset_index[i - 1]);
                    }
                    None => {
                        self.history_subset_index =
                            self.history.get_history_subset(&self.new_buf.to_string());
                        if !self.history_subset_index.is_empty() {
                            self.history_subset_loc = Some(self.history_subset_index.len() - 1);
                            self.cur_history_loc = Some(
                                self.history_subset_index[self.history_subset_index.len() - 1],
                            );
                        }
                    }
                    _ => (),
                }
            } else {
                match self.cur_history_loc {
                    Some(i) if i > 0 => self.cur_history_loc = Some(i - 1),
                    None if !self.history.is_empty() => {
                        self.cur_history_loc = Some(self.history.len() - 1)
                    }
                    _ => (),
                }
            }
            self.hist_buf_valid = false;
            cur_buf_mut!(self);
            self.move_cursor_to_end_of_line()
        }
    }

    /// Move down (forwards) in history, or to the new buffer if we reach the end of history.
    pub fn move_down(&mut self) -> io::Result<()> {
        if self.is_search() {
            self.search(true)
        } else {
            self.hist_buf_valid = false;
            if self.new_buf.num_chars() > 0 {
                if let Some(i) = self.history_subset_loc {
                    if i < self.history_subset_index.len() - 1 {
                        self.history_subset_loc = Some(i + 1);
                        self.cur_history_loc = Some(self.history_subset_index[i + 1]);
                    } else {
                        self.cur_history_loc = None;
                        self.history_subset_loc = None;
                        self.history_subset_index.clear();
                        self.history_fresh = false;
                    }
                }
            } else {
                match self.cur_history_loc.take() {
                    Some(i) if i < self.history.len() - 1 => self.cur_history_loc = Some(i + 1),
                    _ => self.history_fresh = false,
                }
            }
            self.hist_buf_valid = false;
            cur_buf_mut!(self);
            self.move_cursor_to_end_of_line()
        }
    }

    /// Moves to the start of history (ie. the earliest history entry).
    pub fn move_to_start_of_history(&mut self) -> io::Result<()> {
        self.hist_buf_valid = false;
        if self.history.is_empty() {
            self.cur_history_loc = None;
            self.hist_buf_valid = false;
            self.display()
        } else {
            self.cur_history_loc = Some(0);
            self.hist_buf_valid = false;
            cur_buf_mut!(self);
            self.move_cursor_to_end_of_line()
        }
    }

    /// Moves to the end of history (ie. the new buffer).
    pub fn move_to_end_of_history(&mut self) -> io::Result<()> {
        self.hist_buf_valid = false;
        if self.cur_history_loc.is_some() {
            self.cur_history_loc = None;
            self.hist_buf_valid = false;
            self.move_cursor_to_end_of_line()
        } else {
            self.display()
        }
    }

    /// Inserts a string directly after the cursor, moving the cursor to the right.
    ///
    /// Note: it is more efficient to call `insert_chars_after_cursor()` directly.
    pub fn insert_str_after_cursor(&mut self, s: &str) -> io::Result<()> {
        self.insert_chars_after_cursor(&s.chars().collect::<Vec<char>>()[..])
    }

    /// Inserts a character directly after the cursor, moving the cursor to the right.
    pub fn insert_after_cursor(&mut self, c: char) -> io::Result<()> {
        self.insert_chars_after_cursor(&[c])
    }

    /// Inserts characters directly after the cursor, moving the cursor to the right.
    pub fn insert_chars_after_cursor(&mut self, cs: &[char]) -> io::Result<()> {
        {
            let buf = cur_buf_mut!(self);
            buf.insert(self.cursor, cs);
        }

        self.cursor += cs.len();
        self.display()
    }

    /// Deletes the character directly before the cursor, moving the cursor to the left.
    /// If the cursor is at the start of the line, nothing happens.
    pub fn delete_before_cursor(&mut self) -> io::Result<()> {
        if self.cursor > 0 {
            let buf = cur_buf_mut!(self);
            buf.remove(self.cursor - 1, self.cursor);
            self.cursor -= 1;
        }

        self.display()
    }

    /// Deletes the character directly after the cursor. The cursor does not move.
    /// If the cursor is at the end of the line, nothing happens.
    pub fn delete_after_cursor(&mut self) -> io::Result<()> {
        {
            let buf = cur_buf_mut!(self);

            if self.cursor < buf.num_chars() {
                buf.remove(self.cursor, self.cursor + 1);
            }
        }
        self.display()
    }

    /// Deletes every character preceding the cursor until the beginning of the line.
    pub fn delete_all_before_cursor(&mut self) -> io::Result<()> {
        cur_buf_mut!(self).remove(0, self.cursor);
        self.cursor = 0;
        self.display()
    }

    /// Deletes every character after the cursor until the end of the line.
    pub fn delete_all_after_cursor(&mut self) -> io::Result<()> {
        {
            let buf = cur_buf_mut!(self);
            buf.truncate(self.cursor);
        }
        self.display()
    }

    /// Deletes every character from the cursor until the given position.
    pub fn delete_until(&mut self, position: usize) -> io::Result<()> {
        {
            let buf = cur_buf_mut!(self);
            buf.remove(
                cmp::min(self.cursor, position),
                cmp::max(self.cursor, position),
            );
            self.cursor = cmp::min(self.cursor, position);
        }
        self.display()
    }

    /// Deletes every character from the cursor until the given position, inclusive.
    pub fn delete_until_inclusive(&mut self, position: usize) -> io::Result<()> {
        {
            let buf = cur_buf_mut!(self);
            buf.remove(
                cmp::min(self.cursor, position),
                cmp::max(self.cursor + 1, position + 1),
            );
            self.cursor = cmp::min(self.cursor, position);
        }
        self.display()
    }

    /// Moves the cursor to the left by `count` characters.
    /// The cursor will not go past the start of the buffer.
    pub fn move_cursor_left(&mut self, mut count: usize) -> io::Result<()> {
        if count > self.cursor {
            count = self.cursor;
        }

        self.cursor -= count;

        self.display()
    }

    /// Moves the cursor to the right by `count` characters.
    /// The cursor will not go past the end of the buffer.
    pub fn move_cursor_right(&mut self, mut count: usize) -> io::Result<()> {
        {
            let buf = cur_buf!(self);

            if count > buf.num_chars() - self.cursor {
                count = buf.num_chars() - self.cursor;
            }

            self.cursor += count;
        }

        self.display()
    }

    /// Moves the cursor to `pos`. If `pos` is past the end of the buffer, it will be clamped.
    pub fn move_cursor_to(&mut self, pos: usize) -> io::Result<()> {
        self.cursor = pos;
        let buf_len = cur_buf!(self).num_chars();
        if self.cursor > buf_len {
            self.cursor = buf_len;
        }
        self.display()
    }

    /// Moves the cursor to the start of the line.
    pub fn move_cursor_to_start_of_line(&mut self) -> io::Result<()> {
        self.cursor = 0;
        self.display()
    }

    /// Moves the cursor to the end of the line.
    pub fn move_cursor_to_end_of_line(&mut self) -> io::Result<()> {
        self.cursor = cur_buf!(self).num_chars();
        self.display()
    }

    pub fn cursor_is_at_end_of_line(&self) -> bool {
        let num_chars = cur_buf!(self).num_chars();
        if self.no_eol {
            self.cursor == num_chars - 1
        } else {
            self.cursor == num_chars
        }
    }

    ///  Returns a reference to the current buffer being edited.
    /// This may be the new buffer or a buffer from history.
    pub fn current_buffer(&self) -> &Buffer {
        cur_buf!(self)
    }

    ///  Returns a mutable reference to the current buffer being edited.
    /// This may be the new buffer or a buffer from history.
    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        cur_buf_mut!(self)
    }

    /// Accept autosuggestion and copy its content into current buffer
    pub fn accept_autosuggestion(&mut self) -> io::Result<()> {
        if self.show_autosuggestions {
            {
                let autosuggestion = self.autosuggestion.clone();
                let search = self.is_search();
                let buf = self.current_buffer_mut();
                match autosuggestion {
                    Some(ref x) if search => buf.copy_buffer(x),
                    Some(ref x) => buf.insert_from_buffer(x),
                    None => (),
                }
            }
        }
        self.clear_search();
        self.move_cursor_to_end_of_line()
    }

    /// Returns current auto suggestion, for history search this is the current match if not
    /// searching the first history entry to start with current text (reverse order).
    /// Return None if nothing found.
    fn current_autosuggestion(&mut self) -> Option<Buffer> {
        // If we are editing a previous history item no autosuggestion.
        if self.hist_buf_valid || self.new_buf.num_chars() == 0 {
            return None;
        }
        let context_history = &self.history;
        let autosuggestion = if self.is_search() {
            self.search_history_loc().map(|i| &context_history[i])
        } else if self.show_autosuggestions {
            self.cur_history_loc
                .map(|i| &context_history[i])
                .or_else(|| {
                    context_history
                        .get_newest_match(Some(context_history.len()), &self.new_buf.to_string())
                        .map(|i| &context_history[i])
                })
        } else {
            None
        };
        autosuggestion.map(|hist| hist.into())
    }

    pub fn is_currently_showing_autosuggestion(&self) -> bool {
        self.autosuggestion.is_some()
    }

    /// Override the prompt for incremental search if needed.
    fn search_prompt(&mut self) -> String {
        if self.is_search() {
            // If we are searching override prompt to search prompt.
            let (hplace, color) = if self.history_subset_index.is_empty() {
                (0, color::Red.fg_str())
            } else {
                (
                    self.history_subset_loc.unwrap_or(0) + 1,
                    color::Green.fg_str(),
                )
            };
            let prefix = self.prompt.prefix();
            let suffix = self.prompt.suffix();
            format!(
                "{}(search)'{}{}{}` ({}/{}):{} ",
                &prefix,
                color,
                self.current_buffer(),
                color::Reset.fg_str(),
                hplace,
                self.history_subset_index.len(),
                &suffix
            )
        } else {
            self.prompt.to_string()
        }
    }

    fn colorize(&mut self, line: &str) -> String {
        match self.closure {
            Some(ref mut f) if self.use_closure => {
                let color = f(line);
                self.color_lines = Some((line.to_string(), color.clone()));
                color
            }
            Some(_) => {
                if let Some((old_line, colorized)) = &self.color_lines {
                    if line.starts_with(old_line) {
                        let mut new_line = colorized.clone();
                        new_line.push_str(&line[old_line.len()..]);
                        new_line
                    } else {
                        line.to_owned()
                    }
                } else {
                    line.to_owned()
                }
            }
            _ => line.to_owned(),
        }
    }

    fn display_with_suggest(
        &mut self,
        line: &str,
        buf_num_remaining_bytes: usize,
    ) -> io::Result<()> {
        let start = self.colorize(&line[..buf_num_remaining_bytes]);
        if self.is_search() {
            write!(&mut self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
        }
        write!(&mut self.buf, "{}", start).map_err(fmt_io_err)?;
        if !self.is_search() {
            write!(&mut self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
        }
        self.buf.push_str(&line[buf_num_remaining_bytes..]);
        Ok(())
    }

    fn show_lines(&mut self, show_autosuggest: bool, prompt_width: usize) -> io::Result<()> {
        let buf = cur_buf!(self);
        // If we have an autosuggestion, we make the autosuggestion the buffer we print out.
        // We get the number of bytes in the buffer (but NOT the autosuggestion).
        // Then, we loop and subtract from that number until it's 0, in which case we are printing
        // the autosuggestion from here on (in a different color).
        let lines = match self.autosuggestion {
            Some(ref suggestion) if show_autosuggest => suggestion.lines(),
            _ => buf.lines(),
        };
        let mut buf_num_remaining_bytes = buf.num_bytes();

        let lines_len = lines.len();
        for (i, line) in lines.into_iter().enumerate() {
            if i > 0 {
                write!(&mut self.buf, "{}", cursor::Right(prompt_width as u16))
                    .map_err(fmt_io_err)?;
            }

            if buf_num_remaining_bytes == 0 {
                self.buf.push_str(&line);
            } else if line.len() > buf_num_remaining_bytes {
                self.display_with_suggest(&line, buf_num_remaining_bytes)?;
                buf_num_remaining_bytes = 0;
            } else {
                buf_num_remaining_bytes -= line.len();
                let written_line = self.colorize(&line);
                if self.is_search() {
                    write!(&mut self.buf, "{}", color::Yellow.fg_str()).map_err(fmt_io_err)?;
                }
                self.buf.push_str(&written_line);
            }

            if i + 1 < lines_len {
                self.buf.push_str("\r\n");
            }
        }
        Ok(())
    }

    fn _display(&mut self, show_autosuggest: bool) -> io::Result<()> {
        fn calc_width(prompt_width: usize, buf_widths: &[usize], terminal_width: usize) -> usize {
            let mut total = 0;

            for line in buf_widths {
                if total % terminal_width != 0 {
                    total = ((total / terminal_width) + 1) * terminal_width;
                }

                total += prompt_width + line;
            }

            total
        }

        let prompt = self.search_prompt();

        let terminal_width = util::terminal_width()?;
        let prompt_width = util::last_prompt_line_width(&prompt);

        let buf = cur_buf!(self);
        let buf_width = buf.width();

        // Don't let the cursor go over the end!
        let buf_num_chars = buf.num_chars();
        if buf_num_chars < self.cursor {
            self.cursor = buf_num_chars;
        }

        // Can't move past the last character in vi normal mode
        if self.no_eol && self.cursor != 0 && self.cursor == buf_num_chars {
            self.cursor -= 1;
        }

        let buf_widths = match self.autosuggestion {
            Some(ref suggestion) => suggestion.width(),
            None => buf_width,
        };
        // Width of the current buffer lines (including autosuggestion) from the start to the cursor
        let buf_widths_to_cursor = match self.autosuggestion {
            // Cursor might overrun autosuggestion with history search.
            Some(ref suggestion) if self.cursor < suggestion.num_chars() => {
                suggestion.range_width(0, self.cursor)
            }
            _ => buf.range_width(0, self.cursor),
        };

        // Total number of terminal spaces taken up by prompt and buffer
        let new_total_width = calc_width(prompt_width, &buf_widths, terminal_width);
        let new_total_width_to_cursor =
            calc_width(prompt_width, &buf_widths_to_cursor, terminal_width);

        let new_num_lines = (new_total_width + terminal_width) / terminal_width;

        self.buf.push_str("\x1B[?1000l\x1B[?1l");

        // Move the term cursor to the same line as the prompt.
        if self.term_cursor_line > 1 {
            write!(
                &mut self.buf,
                "{}",
                cursor::Up(self.term_cursor_line as u16 - 1)
            )
            .map_err(fmt_io_err)?;
        }

        write!(&mut self.buf, "\r{}", clear::AfterCursor).map_err(fmt_io_err)?;

        // If we're cycling through completions, show those
        let mut completion_lines = 0;
        if let Some((completions, i)) = self.show_completions_hint.as_ref() {
            completion_lines = 1 + Self::print_completion_list(completions, *i, &mut self.buf)?;
            self.buf.push_str("\r\n");
        }

        // Write the prompt
        write!(&mut self.buf, "{}", prompt).map_err(fmt_io_err)?;

        self.show_lines(show_autosuggest, prompt_width)?;

        // at the end of the line, move the cursor down a line
        if new_total_width % terminal_width == 0 {
            self.buf.push_str("\r\n");
        }

        self.term_cursor_line = (new_total_width_to_cursor + terminal_width) / terminal_width;

        // The term cursor is now on the bottom line. We may need to move the term cursor up
        // to the line where the true cursor is.
        let cursor_line_diff = new_num_lines as isize - self.term_cursor_line as isize;
        match cursor_line_diff.cmp(&0) {
            Ordering::Greater => write!(&mut self.buf, "{}", cursor::Up(cursor_line_diff as u16))
                .map_err(fmt_io_err)?,
            Ordering::Less => unreachable!(),
            Ordering::Equal => {}
        }

        // Now that we are on the right line, we must move the term cursor left or right
        // to match the true cursor.
        let cursor_col_diff = new_total_width as isize
            - new_total_width_to_cursor as isize
            - cursor_line_diff * terminal_width as isize;
        match cursor_col_diff.cmp(&0) {
            Ordering::Greater => write!(&mut self.buf, "{}", cursor::Left(cursor_col_diff as u16))
                .map_err(fmt_io_err)?,
            Ordering::Less => {
                write!(
                    &mut self.buf,
                    "{}",
                    cursor::Right((-cursor_col_diff) as u16)
                )
                .map_err(fmt_io_err)?;
            }
            Ordering::Equal => {}
        }
        self.term_cursor_line += completion_lines;

        {
            let out = &mut self.out;
            write!(
                &mut self.buf,
                "{}{}",
                color::Reset.fg_str(),
                color::Reset.bg_str()
            )
            .map_err(fmt_io_err)?;
            out.write_all(self.buf.as_bytes())?;
            self.buf.clear();
            out.flush()
        }
    }

    /// Deletes the displayed prompt and buffer, replacing them with the current prompt and buffer
    pub fn display(&mut self) -> io::Result<()> {
        if self.is_search() && self.buffer_changed {
            // Refresh incremental search.
            let forward = self.forward_search;
            self.refresh_search(forward);
        }
        self.autosuggestion = self.current_autosuggestion();

        self._display(true)
    }

    /// Modifies the prompt prefix.
    /// Useful to reflect a keybinding mode (vi insert/normal for instance).
    pub fn set_prompt_prefix<P: Into<String>>(&mut self, prefix: P) {
        self.prompt.prefix = Some(prefix.into());
    }

    /// Clears the prompt prefix.
    /// Useful to reflect a keybinding mode (vi insert/normal for instance).
    pub fn clear_prompt_prefix(&mut self) {
        self.prompt.prefix = None;
    }

    /// Modifies the prompt suffix.
    /// Useful to reflect a keybinding mode (vi insert/normal for instance).
    pub fn set_prompt_suffix<S: Into<String>>(&mut self, suffix: S) {
        self.prompt.suffix = Some(suffix.into());
    }

    /// Clears the prompt prefix.
    /// Useful to reflect a keybinding mode (vi insert/normal for instance).
    pub fn clear_prompt_suffix(&mut self) {
        self.prompt.suffix = None;
    }
}

impl<'a> From<Editor<'a>> for String {
    fn from(ed: Editor<'a>) -> String {
        match ed.cur_history_loc {
            Some(i) => {
                if ed.hist_buf_valid {
                    ed.hist_buf
                } else {
                    ed.history[i].into()
                }
            }
            _ => ed.new_buf,
        }
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::get_buffer_words;
    use crate::History;

    #[test]
    /// test undoing delete_all_after_cursor
    fn delete_all_after_cursor_undo() {
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

        ed.insert_str_after_cursor("delete all of this").unwrap();
        ed.move_cursor_to_start_of_line().unwrap();
        ed.delete_all_after_cursor().unwrap();
        ed.undo().unwrap();
        assert_eq!(String::from(ed), "delete all of this");
    }

    #[test]
    fn move_cursor_left() {
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
        ed.insert_str_after_cursor("let").unwrap();
        assert_eq!(ed.cursor, 3);

        ed.move_cursor_left(1).unwrap();
        assert_eq!(ed.cursor, 2);

        ed.insert_after_cursor('f').unwrap();
        assert_eq!(ed.cursor, 3);
        assert_eq!(String::from(ed), "left");
    }

    #[test]
    fn cursor_movement() {
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
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor, 5);

        ed.move_cursor_left(2).unwrap();
        ed.move_cursor_right(1).unwrap();
        assert_eq!(ed.cursor, 4);
    }

    #[test]
    fn delete_until_backwards() {
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
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor, 5);

        ed.delete_until(0).unwrap();
        assert_eq!(ed.cursor, 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    fn delete_until_forwards() {
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
        ed.insert_str_after_cursor("right").unwrap();
        ed.cursor = 0;

        ed.delete_until(5).unwrap();
        assert_eq!(ed.cursor, 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    fn delete_until() {
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
        ed.insert_str_after_cursor("right").unwrap();
        ed.cursor = 4;

        ed.delete_until(1).unwrap();
        assert_eq!(ed.cursor, 1);
        assert_eq!(String::from(ed), "rt");
    }

    #[test]
    fn delete_until_inclusive() {
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
        ed.insert_str_after_cursor("right").unwrap();
        ed.cursor = 4;

        ed.delete_until_inclusive(1).unwrap();
        assert_eq!(ed.cursor, 1);
        assert_eq!(String::from(ed), "r");
    }
}
