use std::io;

use sl_console::{self, color};

use crate::context::ColorClosure;
use crate::cursor::CursorPosition;
use crate::editor_rules::last_non_ws_char_was_not_backslash;
use crate::event::*;
use crate::prompt::Prompt;
use crate::{util, EditorRules, Terminal};
use crate::{Buffer, Cursor};
use crate::{History, Metrics};

use super::complete::Completer;

/// The core line editor. Displays and provides editing for history and the new buffer.
pub struct Editor<'a> {
    prompt: Prompt,
    history: &'a mut History,
    //TODO rename
    editor_rules: Option<&'a dyn EditorRules>,

    // w/ buffer and pos/count directives maintain the location of the terminal's
    // cursor
    cursor: Cursor<'a>,

    // Buffer for the new line (ie. not from editing history)
    new_buf: Buffer,

    // Buffer to use when editing history so we do not overwrite it.
    hist_buf: Buffer,
    hist_buf_valid: bool,

    // None if we're on the new buffer, else the index of history
    cur_history_loc: Option<usize>,

    // Terminal is the interface editor uses to write to the actual terminal.
    term: Terminal<'a>,

    // The next completion to suggest, or none
    show_completions_hint: Option<(Vec<String>, Option<usize>)>,

    // Show autosuggestions based on history
    show_autosuggestions: bool,

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
            _ => &$s.new_buf,
        }
    };
}

impl<'a> Editor<'a> {
    pub fn new(
        out: &'a mut dyn io::Write,
        prompt: Prompt,
        f: Option<ColorClosure>,
        history: &'a mut History,
        buf: &'a mut String,
        editor_rules: Option<&'a dyn EditorRules>,
    ) -> io::Result<Self> {
        Editor::new_with_init_buffer(out, prompt, f, history, buf, Buffer::new(), editor_rules)
    }

    pub fn new_with_init_buffer<B: Into<Buffer>>(
        out: &'a mut dyn io::Write,
        prompt: Prompt,
        f: Option<ColorClosure>,
        history: &'a mut History,
        buf: &'a mut String,
        buffer: B,
        editor_rules: Option<&'a dyn EditorRules>,
    ) -> io::Result<Self> {
        let mut term = Terminal::new(f, buf, out);
        let prompt = term.make_prompt(prompt)?;
        let mut ed = Editor {
            prompt,
            editor_rules,
            cursor: Cursor::new_with_divider(editor_rules),
            new_buf: buffer.into(),
            hist_buf: Buffer::new(),
            hist_buf_valid: false,
            cur_history_loc: None,
            history,
            show_completions_hint: None,
            show_autosuggestions: true,
            term,
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
        } else {
            ed.display_term()?;
        }
        Ok(ed)
    }

    pub fn set_closure(&mut self, closure: ColorClosure) -> &mut Self {
        self.term.set_closure(closure);
        self
    }

    pub fn use_closure(&mut self, use_closure: bool) {
        self.term.use_closure(use_closure)
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
        self.cursor.get_words_and_cursor_position(cur_buf!(self))
    }

    pub fn history(&mut self) -> &mut History {
        self.history
    }

    pub fn cursor(&self) -> usize {
        self.cursor.curr_grapheme()
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

        let buf = cur_buf_mut!(self);
        let should_evaluate = if let Some(editor_rules) = self.editor_rules {
            editor_rules.evaluate_on_newline(buf)
        } else {
            last_non_ws_char_was_not_backslash(buf)
        };
        if should_evaluate {
            self.cursor.move_cursor_to_end_of_line(cur_buf!(self));
            self.display_term_with_autosuggest(false)?;
            self.term.write_newline()?;
            self.show_completions_hint = None;
            Ok(true)
        } else {
            buf.push('\n');
            self.cursor.move_cursor_to_end_of_line(buf);
            self.display_term()?;
            Ok(false)
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
        self.display_term()?;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.term.flush()
    }

    /// Attempts to undo an action on the current buffer.
    ///
    /// Returns `Ok(true)` if an action was undone.
    /// Returns `Ok(false)` if there was no action to undo.
    pub fn undo(&mut self) -> Option<usize> {
        cur_buf_mut!(self).undo()
    }

    pub fn redo(&mut self) -> Option<usize> {
        cur_buf_mut!(self).redo()
    }

    /// Inserts characters from internal register to the right or the left of the cursor, moving the
    /// cursor to the last character inserted.
    pub fn paste(&mut self, right: bool, count: usize) -> io::Result<()> {
        self.cursor.insert_around(cur_buf_mut!(self), right, count);
        self.display_term()
    }

    pub fn revert(&mut self) -> io::Result<bool> {
        let did = cur_buf_mut!(self).revert();
        if did {
            self.move_cursor_to_end_of_line()?;
        } else {
            self.display_term()?;
        }
        Ok(did)
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
                    self.cursor.reset(cur_buf_mut!(self));
                }
                _ => self.delete_word_before_cursor(false)?,
            }
            self.insert_str_after_cursor(&completions[i])?;

            self.show_completions_hint = Some((completions, Some(i)));
        }
        if self.show_completions_hint.is_some() {
            self.display_term()?;
            return Ok(());
        }

        let (word, completions) = {
            let word_range = self.get_word_before_cursor(false);
            let buf = cur_buf_mut!(self);

            let word = match word_range {
                Some((start, end)) => buf.range_graphemes(start, end).slice(),
                None => "",
            };

            let mut completions = handler.completions(word);
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

                if s.len() > word.len() && s.starts_with(word) {
                    self.delete_word_before_cursor(false)?;
                    return self.insert_str_after_cursor(s.as_ref());
                }
            }

            self.show_completions_hint = Some((completions, None));
            self.display_term()?;

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
            self.cursor.delete_until_cursor(cur_buf_mut!(self), start);
        }
        self.display_term()
    }

    /// Clears the screen then prints the prompt and current buffer.
    pub fn clear(&mut self) -> io::Result<()> {
        self.term.clear()?;
        self.clear_search();
        self.display_term()
    }

    /// Move up (backwards) in history.
    pub fn move_up(&mut self) -> io::Result<()> {
        if self.is_search() {
            self.search(false)
        } else {
            self.hist_buf_valid = false;
            self.freshen_history();
            if self.new_buf.num_graphemes() > 0 {
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
            if self.new_buf.num_graphemes() > 0 {
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
            self.display_term()
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
            self.display_term()
        }
    }

    pub fn flip_case(&mut self) -> io::Result<()> {
        let cursor = self.cursor();
        let buf_mut = cur_buf_mut!(self);
        let str = buf_mut.grapheme_after(cursor).map(String::from);
        if let Some(str) = str {
            let mut c = str.chars();
            match c.next() {
                Some(f) if f.is_lowercase() => {
                    self.cursor.delete_after_cursor(buf_mut);
                    self.insert_str_after_cursor(
                        &*(f.to_uppercase().collect::<String>() + c.as_str()),
                    )
                }
                Some(f) if f.is_uppercase() => {
                    self.cursor.delete_after_cursor(buf_mut);
                    self.insert_str_after_cursor(
                        &*(f.to_lowercase().collect::<String>() + c.as_str()),
                    )
                }
                _ => self.move_cursor_right(1),
            }
        } else {
            self.move_cursor_right(1)
        }
    }

    /// Inserts a string directly after the cursor, moving the cursor to the right.
    ///
    /// Note: it is more efficient to call `insert_chars_after_cursor()` directly.
    pub fn insert_str_after_cursor(&mut self, s: &str) -> io::Result<()> {
        self.cursor.insert_str_after_cursor(cur_buf_mut!(self), s);
        self.display_term()
    }

    /// Inserts a character directly after the cursor, moving the cursor to the right.
    pub fn insert_after_cursor(&mut self, c: char) -> io::Result<()> {
        self.cursor.insert_char_after_cursor(cur_buf_mut!(self), c);
        self.display_term()
    }

    /// Inserts characters directly after the cursor, moving the cursor to the right.
    pub fn insert_chars_after_cursor(&mut self, cs: &[char]) -> io::Result<()> {
        self.cursor
            .insert_chars_after_cursor(cur_buf_mut!(self), cs);
        self.display_term()
    }

    /// Deletes the character directly before the cursor, moving the cursor to the left.
    /// If the cursor is at the start of the line, nothing happens.
    pub fn delete_before_cursor(&mut self) -> io::Result<()> {
        self.cursor.delete_before_cursor(cur_buf_mut!(self));
        self.display_term()
    }

    /// Deletes the character directly after the cursor. The cursor does not move.
    /// If the cursor is at the end of the line, nothing happens.
    pub fn delete_after_cursor(&mut self) -> io::Result<()> {
        self.cursor.delete_after_cursor(cur_buf_mut!(self));
        self.display_term()
    }

    /// Deletes every character preceding the cursor until the beginning of the line.
    pub fn delete_all_before_cursor(&mut self) -> io::Result<()> {
        self.cursor.delete_all_before_cursor(cur_buf_mut!(self));
        self.display_term()
    }

    /// Yanks every character after the cursor until the end of the line.
    pub fn yank_all_after_cursor(&mut self) -> io::Result<()> {
        self.cursor.yank_all_after_cursor(cur_buf_mut!(self));
        self.display_term()
    }

    /// Deletes every character after the cursor until the end of the line.
    pub fn delete_all_after_cursor(&mut self) -> io::Result<()> {
        self.cursor.delete_all_after_cursor(cur_buf_mut!(self));
        self.display_term()
    }

    /// Yanks every character from the cursor until the given position.
    pub fn yank_until(&mut self, position: usize) -> io::Result<()> {
        self.cursor.yank_until(cur_buf_mut!(self), position);
        self.display_term()
    }

    /// Deletes every character from the cursor until the given position. Does not register as an
    /// action in the undo/redo buffer or in the buffer's register.
    pub fn delete_until_silent(&mut self, position: usize) -> io::Result<()> {
        self.cursor
            .delete_until_silent(cur_buf_mut!(self), position);
        self.display_term()
    }

    /// Deletes every character from the cursor until the given position.
    pub fn delete_until(&mut self, position: usize) -> io::Result<()> {
        self.cursor.delete_until(cur_buf_mut!(self), position);
        self.display_term()
    }

    /// Yanks every character from the cursor until the given position, inclusive.
    pub fn yank_until_inclusive(&mut self, position: usize) -> io::Result<()> {
        self.cursor
            .yank_until_inclusive(cur_buf_mut!(self), position);
        self.display_term()
    }

    /// Deletes every character from the cursor until the given position, inclusive.
    pub fn delete_until_inclusive(&mut self, position: usize) -> io::Result<()> {
        self.cursor
            .delete_until_inclusive(cur_buf_mut!(self), position);
        self.display_term()
    }

    /// Moves the cursor to the left by `count` characters.
    /// The cursor will not go past the start of the buffer.
    pub fn move_cursor_left(&mut self, count: usize) -> io::Result<()> {
        self.cursor.move_cursor_left(count);
        self.display_term()
    }

    /// Moves the cursor to the right by `count` characters.
    /// The cursor will not go past the end of the buffer.
    pub fn move_cursor_right(&mut self, count: usize) -> io::Result<()> {
        self.cursor.move_cursor_right(cur_buf!(self), count);
        self.display_term()
    }

    /// Moves the cursor to `pos`. If `pos` is past the end of the buffer, it will be clamped.
    pub fn move_cursor_to(&mut self, pos: usize) -> io::Result<()> {
        self.cursor.move_cursor_to(cur_buf!(self), pos);
        self.display_term()
    }

    /// Moves the cursor to the start of the line.
    pub fn move_cursor_to_start_of_line(&mut self) -> io::Result<()> {
        self.cursor.move_cursor_to(cur_buf!(self), 0);
        self.display_term()
    }

    /// Moves the cursor to the end of the line.
    pub fn move_cursor_to_end_of_line(&mut self) -> io::Result<()> {
        self.cursor.move_cursor_to_end_of_line(cur_buf!(self));
        self.display_term()
    }

    pub fn curr_char(&self) -> Option<&str> {
        let buf = cur_buf!(self);
        buf.grapheme_after(self.cursor.curr_grapheme())
    }

    pub fn is_cursor_at_beginning_of_word_or_line(&self) -> bool {
        self.cursor.is_at_beginning_of_word_or_line(cur_buf!(self))
    }

    pub fn is_cursor_at_end_of_line(&self) -> bool {
        self.cursor.is_at_end_of_line(cur_buf!(self))
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
                    Some(ref x) if search => {
                        buf.copy_buffer(x);
                    }
                    Some(ref x) => {
                        buf.append_buffer(x);
                    }
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
        if self.hist_buf_valid || self.new_buf.num_graphemes() == 0 {
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
    fn get_prompt(&self) -> String {
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

    pub fn set_no_eol(&mut self, no_eol: bool) {
        self.cursor.set_no_eol(no_eol);
    }

    fn display_term_with_autosuggest(&mut self, show_autosuggest: bool) -> io::Result<()> {
        let prompt = self.get_prompt();
        let buf = cur_buf!(self);
        let is_search = self.is_search();

        let metrics = Metrics::new(&prompt, buf, &self.cursor, self.autosuggestion.as_ref())?;
        self.cursor.pre_display_adjustment(buf);
        self.term.clear_after_cursor()?;
        let completion_lines = self
            .term
            .maybe_write_completions(self.show_completions_hint.as_ref())?;

        // Write the prompt
        self.term.write_prompt(&prompt)?;

        self.term.show_lines(
            buf,
            self.autosuggestion.as_ref(),
            show_autosuggest,
            metrics,
            is_search,
        )?;

        self.term.display(metrics, completion_lines)?;

        Ok(())
    }

    /// Deletes the displayed prompt and buffer, replacing them with the current prompt and buffer
    pub fn display_term(&mut self) -> io::Result<()> {
        if self.is_search() && self.buffer_changed {
            // Refresh incremental search.
            let forward = self.forward_search;
            self.refresh_search(forward);
        }
        self.autosuggestion = self.current_autosuggestion();

        self.display_term_with_autosuggest(true)
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
    use crate::prompt::Prompt;
    use crate::History;

    use super::*;

    #[test]
    /// test undoing delete_all_after_cursor
    fn delete_all_after_cursor_undo() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();

        ed.insert_str_after_cursor("delete all of this").unwrap();
        ed.move_cursor_to_start_of_line().unwrap();
        ed.delete_all_after_cursor().unwrap();
        ed.undo().unwrap();
        assert_eq!(String::from(ed), "delete all of this");
    }

    #[test]
    fn move_cursor_multiline() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("let\\").unwrap();
        assert_eq!(ed.cursor(), 4);
        let done = ed.handle_newline();
        assert!(!done.unwrap());
        ed.insert_str_after_cursor("\\\n").unwrap();
        assert_eq!(ed.cursor(), 7);
        let done = ed.handle_newline();
        assert!(done.unwrap());
    }

    #[test]
    fn move_cursor_left() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("let").unwrap();
        assert_eq!(ed.cursor(), 3);

        ed.move_cursor_left(1).unwrap();
        assert_eq!(ed.cursor(), 2);

        ed.insert_after_cursor('f').unwrap();
        assert_eq!(ed.cursor(), 3);
        assert_eq!(String::from(ed), "left");
    }

    #[test]
    fn test_handle_newline() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();

        ed.insert_str_after_cursor("oneline").unwrap();
        assert_eq!(ed.cursor(), 7);

        let done = ed.handle_newline();
        assert!(done.unwrap());
    }

    #[test]
    fn cursor_movement() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        ed.move_cursor_left(2).unwrap();
        ed.move_cursor_right(1).unwrap();
        assert_eq!(ed.cursor(), 4);
    }

    #[test]
    fn delete_until_backwards() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 5);

        ed.delete_until(0).unwrap();
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    fn delete_until_forwards() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        ed.move_cursor_to_start_of_line().unwrap();

        ed.delete_until(5).unwrap();
        assert_eq!(ed.cursor(), 0);
        assert_eq!(String::from(ed), "");
    }

    #[test]
    fn delete_until() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        ed.move_cursor_left(1).unwrap();

        ed.delete_until(1).unwrap();
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "rt");
    }

    #[test]
    fn delete_until_inclusive() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let mut ed = Editor::new(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        ed.move_cursor_left(1).unwrap();

        ed.delete_until_inclusive(1).unwrap();
        assert_eq!(ed.cursor(), 1);
        assert_eq!(String::from(ed), "r");
    }

    #[test]
    fn test_cursor_when_init_buffer_is_not_empty() {
        let mut out = Vec::new();
        let mut history = History::new();
        let mut buf = String::with_capacity(512);
        let buffer = Buffer::from("\u{1f469}\u{200d}\u{1f4bb} start here_".to_owned());
        let mut ed = Editor::new_with_init_buffer(
            &mut out,
            Prompt::from("prompt"),
            None,
            &mut history,
            &mut buf,
            buffer,
            None,
        )
        .unwrap();
        ed.insert_str_after_cursor("right").unwrap();
        assert_eq!(ed.cursor(), 18);
        assert_eq!(
            "\u{1f469}\u{200d}\u{1f4bb} start here_right",
            String::from(ed)
        );
    }
}
