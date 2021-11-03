use crate::Buffer;
use std::cmp;

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

#[derive(Clone)]
pub struct Cursor<'a> {
    // The location of the cursor. Note that the cursor does not lie on a char, but between chars.
    // So, if `cursor == 0` then the cursor is before the first char,
    // and if `cursor == 1` ten the cursor is after the first char and before the second char.
    char_vec_pos: usize,
    //TODO doc
    word_divider_fn: &'a dyn Fn(&Buffer) -> Vec<(usize, usize)>,

    // if set, the cursor will not be allow to move one past the end of the line, this is necessary
    // for Vi's normal mode.
    pub no_eol: bool,
}

impl<'a> Cursor<'a> {
    pub fn new(word_divider_fn: &'a dyn Fn(&Buffer) -> Vec<(usize, usize)>) -> Self {
        Cursor {
            char_vec_pos: 0,
            word_divider_fn,
            no_eol: false,
        }
    }

    pub fn set_char_vec_pos(&mut self, pos: usize) {
        self.char_vec_pos = pos;
    }

    pub fn char_vec_pos(&self) -> usize {
        self.char_vec_pos
    }

    pub fn insert_around(&mut self, buf: &mut Buffer, right: bool, count: usize) {
        let delta = buf.insert_register_around_start(self.char_vec_pos, count, right);
        if delta > 0 {
            // if moving to the left we move one less than the number of chars inserted because
            // the cursor rests on the last character inserted.
            let adjustment = if right { delta } else { delta - 1 };
            self.move_cursor_to(buf, self.char_vec_pos + adjustment)
        }
    }

    /// Moves the cursor to `pos`. If `pos` is past the end of the buffer, it will be clamped.
    pub fn move_cursor_to(&mut self, buf: &Buffer, pos: usize) {
        self.char_vec_pos = pos;
        let buf_len = buf.num_chars();
        if self.char_vec_pos > buf_len {
            self.char_vec_pos = buf_len;
        }
    }

    pub fn get_words_and_cursor_position(
        &self,
        buf: &Buffer,
    ) -> (Vec<(usize, usize)>, CursorPosition) {
        let word_fn = &self.word_divider_fn;
        let words = word_fn(buf);
        let pos = CursorPosition::get(self.char_vec_pos, &words);
        (words, pos)
    }

    pub fn reset(&mut self, buf: &mut Buffer) {
        self.char_vec_pos = 0;
        buf.truncate(0);
    }

    pub fn remove(&mut self, buf: &mut Buffer, start: usize) {
        let moved = buf.remove(start, self.char_vec_pos);
        self.char_vec_pos -= moved;
    }

    /*
            //TODO take this out
    use unicode_segmentation::UnicodeSegmentation;
            let text: String = text.iter().collect();
            UnicodeSegmentation::graphemes(&text[..], true).count()
            [(0, 1)
            (1, 1)
            (2, 1)
            (3, 2)
            (5, 1)]

             [ 'a' 'b' 'c' "ते", 'd']
         */
    pub fn insert_char_after_cursor(&mut self, buf: &mut Buffer, c: char) {
        let _len = buf.insert(self.char_vec_pos, &[c]);
        self.char_vec_pos += 1;
    }

    pub fn insert_str_after_cursor(&mut self, buf: &mut Buffer, s: &str) {
        let cs = &s.chars().collect::<Vec<char>>()[..];
        let _len = buf.insert(self.char_vec_pos, cs);
        self.char_vec_pos += cs.len();
    }

    pub fn insert_chars_after_cursor(&mut self, buf: &mut Buffer, cs: &[char]) {
        let _len = buf.insert(self.char_vec_pos, cs);
        self.char_vec_pos += cs.len();
    }

    pub fn delete_before_cursor(&mut self, buf: &mut Buffer) {
        if self.char_vec_pos > 0 {
            buf.remove(self.char_vec_pos - 1, self.char_vec_pos);
            self.char_vec_pos -= 1;
        }
    }

    pub fn delete_after_cursor(&mut self, buf: &mut Buffer) {
        if self.char_vec_pos < buf.num_chars() {
            buf.remove(self.char_vec_pos, self.char_vec_pos + 1);
        }
    }

    pub fn delete_all_before_cursor(&mut self, buf: &mut Buffer) {
        buf.remove(0, self.char_vec_pos);
        self.char_vec_pos = 0;
    }

    pub fn yank_all_after_cursor(&mut self, buf: &mut Buffer) {
        buf.yank(self.char_vec_pos, buf.num_chars());
    }

    pub fn delete_all_after_cursor(&mut self, buf: &mut Buffer) {
        buf.truncate(self.char_vec_pos);
    }

    pub fn yank_until(&mut self, buf: &mut Buffer, position: usize) {
        buf.yank(
            cmp::min(self.char_vec_pos, position),
            cmp::max(self.char_vec_pos, position),
        );
    }

    pub fn delete_until_silent(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove_silent(
            cmp::min(self.char_vec_pos, position),
            cmp::max(self.char_vec_pos, position),
        );
        self.char_vec_pos = cmp::min(self.char_vec_pos, position);
    }

    pub fn delete_until(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove(
            cmp::min(self.char_vec_pos, position),
            cmp::max(self.char_vec_pos, position),
        );
        self.char_vec_pos = cmp::min(self.char_vec_pos, position);
    }

    pub fn yank_until_inclusive(&mut self, buf: &mut Buffer, position: usize) {
        buf.yank(
            cmp::min(self.char_vec_pos, position),
            cmp::max(self.char_vec_pos + 1, position + 1),
        );
    }

    pub fn delete_until_inclusive(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove(
            cmp::min(self.char_vec_pos, position),
            cmp::max(self.char_vec_pos + 1, position + 1),
        );
        self.char_vec_pos = cmp::min(self.char_vec_pos, position);
    }

    pub fn move_cursor_left(&mut self, count: usize) {
        let mut inc = count;
        if count > self.char_vec_pos {
            inc = self.char_vec_pos;
        }
        self.char_vec_pos -= inc;
    }

    pub fn move_cursor_right(&mut self, buf: &Buffer, count: usize) {
        let mut inc = count;
        if count > buf.num_chars() - self.char_vec_pos {
            inc = buf.num_chars() - self.char_vec_pos;
        }

        self.char_vec_pos += inc;
    }

    pub fn move_cursor_to_end_of_line(&mut self, buf: &Buffer) {
        self.char_vec_pos = buf.num_chars();
    }

    pub fn is_at_beginning_of_word_or_line(&self, buf: &Buffer) -> bool {
        let num_chars = buf.num_chars();
        let cursor_pos = self.char_vec_pos;
        if num_chars > 0 && cursor_pos != 0 {
            let c = buf.char_before(cursor_pos);
            if let Some(c) = c {
                return c.is_whitespace();
            }
        }
        true
    }

    pub fn set_no_eol(&mut self, no_eol: bool) {
        self.no_eol = no_eol;
    }

    pub fn is_at_end_of_line(&self, buf: &Buffer) -> bool {
        let num_chars = buf.num_chars();
        if self.no_eol {
            self.char_vec_pos == num_chars - 1
        } else {
            self.char_vec_pos == num_chars
        }
    }

    pub fn pre_display_adjustment(&mut self, buf: &Buffer) {
        let buf_num_chars = buf.num_chars();
        // Don't let the cursor go over the end!
        if buf_num_chars < self.char_vec_pos {
            self.char_vec_pos = buf_num_chars;
        }

        // Can't move past the last character in vi normal mode
        if self.no_eol && self.char_vec_pos != 0 && self.char_vec_pos == buf_num_chars {
            self.char_vec_pos -= 1;
        }
    }
}
