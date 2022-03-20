//! Track current grapheme offset for terminal cursor
use std::cmp;

use crate::{Buffer, EditorRules};

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

/// Cursor (as in the terminal's cursor) exposes various functions that modify the
/// Buffer (Buffer for new line or for editing history, that's transparent to Cursor)
/// and maintain the position on the terminal that the actual cursor needs to be
/// drawn.
#[derive(Clone)]
pub struct Cursor<'a, T: EditorRules> {
    // The location of the cursor. Note that the cursor does not lie on a char, but between chars.
    // So, if `cursor == 0` then the cursor is before the first char,
    // and if `cursor == 1` ten the cursor is after the first char and before the second char.
    curr_grapheme: usize,
    // function to determine how to split words, returns vector of tuples representing index
    // and length of word.
    word_divider_fn: &'a T,

    // if set, the cursor will not be allow to move one past the end of the line, this is necessary
    // for Vi's normal mode.
    pub no_eol: bool,
}

impl<'a, T> Cursor<'a, T> where T: EditorRules {
    pub fn new_with_divider(divider: &'a T) -> Self {
        Cursor {
            curr_grapheme: 0,
            word_divider_fn: divider,
            no_eol: false,
        }
    }

    pub fn curr_grapheme(&self) -> usize {
        self.curr_grapheme
    }

    pub fn insert_around(&mut self, buf: &mut Buffer, right: bool, count: usize) {
        let delta = buf.insert_register_around_idx(self.curr_grapheme, count, right);
        if delta > 0 {
            // if moving to the left we move one less than the number of chars inserted because
            // the cursor rests on the last character inserted.
            let adjustment = if right { delta } else { delta - 1 };
            self.move_cursor_to(buf, self.curr_grapheme + adjustment)
        }
    }

    /// Moves the cursor to `pos`. If `pos` is past the end of the buffer, it will be clamped.
    pub fn move_cursor_to(&mut self, buf: &Buffer, pos: usize) {
        self.curr_grapheme = pos;
        let buf_len = buf.num_graphemes();
        if self.curr_grapheme > buf_len {
            self.curr_grapheme = buf_len;
        }
    }

    pub fn get_words_and_cursor_position(
        &self,
        buf: &Buffer,
    ) -> (Vec<(usize, usize)>, CursorPosition) {
        let words = self.word_divider_fn.divide_words(buf);
        let pos = CursorPosition::get(self.curr_grapheme, &words);
        (words, pos)
    }

    pub fn reset(&mut self, buf: &mut Buffer) {
        self.curr_grapheme = 0;
        buf.truncate(0);
    }

    pub fn delete_until_cursor(&mut self, buf: &mut Buffer, start: usize) {
        let moved = buf.remove(start, self.curr_grapheme);
        self.curr_grapheme -= moved;
    }

    pub fn insert_char_after_cursor(&mut self, buf: &mut Buffer, c: char) {
        let len = buf.insert(self.curr_grapheme, [c].iter());
        self.curr_grapheme += len;
    }

    pub fn insert_str_after_cursor(&mut self, buf: &mut Buffer, s: &str) {
        let cs = &s.chars().collect::<Vec<char>>()[..];
        let len = buf.insert(self.curr_grapheme, cs.iter());
        self.curr_grapheme += len;
    }

    pub fn insert_chars_after_cursor(&mut self, buf: &mut Buffer, cs: &[char]) {
        let len = buf.insert(self.curr_grapheme, cs.iter());
        self.curr_grapheme += len;
    }

    pub fn delete_before_cursor(&mut self, buf: &mut Buffer) {
        if self.curr_grapheme > 0 {
            buf.remove(self.curr_grapheme - 1, self.curr_grapheme);
            self.curr_grapheme -= 1;
        }
    }

    pub fn delete_after_cursor(&mut self, buf: &mut Buffer) {
        if self.curr_grapheme < buf.num_graphemes() {
            buf.remove(self.curr_grapheme, self.curr_grapheme + 1);
        }
    }

    pub fn delete_all_before_cursor(&mut self, buf: &mut Buffer) {
        buf.remove(0, self.curr_grapheme);
        self.curr_grapheme = 0;
    }

    pub fn yank_all_after_cursor(&mut self, buf: &mut Buffer) {
        buf.yank(self.curr_grapheme, buf.num_graphemes());
    }

    pub fn delete_all_after_cursor(&mut self, buf: &mut Buffer) {
        buf.truncate(self.curr_grapheme);
    }

    pub fn yank_until(&mut self, buf: &mut Buffer, position: usize) {
        buf.yank(
            cmp::min(self.curr_grapheme, position),
            cmp::max(self.curr_grapheme, position),
        );
    }

    pub fn delete_until_silent(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove_unrecorded(
            cmp::min(self.curr_grapheme, position),
            cmp::max(self.curr_grapheme, position),
        );
        self.curr_grapheme = cmp::min(self.curr_grapheme, position);
    }

    pub fn delete_until(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove(
            cmp::min(self.curr_grapheme, position),
            cmp::max(self.curr_grapheme, position),
        );
        self.curr_grapheme = cmp::min(self.curr_grapheme, position);
    }

    pub fn yank_until_inclusive(&mut self, buf: &mut Buffer, position: usize) {
        buf.yank(
            cmp::min(self.curr_grapheme, position),
            cmp::max(self.curr_grapheme + 1, position + 1),
        );
    }

    pub fn delete_until_inclusive(&mut self, buf: &mut Buffer, position: usize) {
        buf.remove(
            cmp::min(self.curr_grapheme, position),
            cmp::max(self.curr_grapheme + 1, position + 1),
        );
        self.curr_grapheme = cmp::min(self.curr_grapheme, position);
    }

    pub fn move_cursor_left(&mut self, count: usize) {
        let mut inc = count;
        if count > self.curr_grapheme {
            inc = self.curr_grapheme;
        }
        self.curr_grapheme -= inc;
    }

    pub fn move_cursor_right(&mut self, buf: &Buffer, count: usize) {
        let mut inc = count;
        if count > buf.num_graphemes() - self.curr_grapheme {
            inc = buf.num_graphemes() - self.curr_grapheme;
        }

        self.curr_grapheme += inc;
    }

    pub fn move_cursor_to_end_of_line(&mut self, buf: &Buffer) {
        self.curr_grapheme = buf.num_graphemes();
    }

    pub fn is_at_beginning_of_word_or_line(&self, buf: &Buffer) -> bool {
        let num_chars = buf.num_graphemes();
        let cursor_pos = self.curr_grapheme;
        if num_chars > 0 && cursor_pos != 0 {
            let str = buf.grapheme_before(cursor_pos);
            if let Some(str) = str {
                return str.trim().is_empty();
            }
        }
        true
    }

    pub fn set_no_eol(&mut self, no_eol: bool) {
        self.no_eol = no_eol;
    }

    pub fn is_at_end_of_line(&self, buf: &Buffer) -> bool {
        let num_chars = buf.num_graphemes();
        if self.no_eol {
            self.curr_grapheme == num_chars - 1
        } else {
            self.curr_grapheme == num_chars
        }
    }

    pub fn pre_display_adjustment(&mut self, buf: &Buffer) {
        let buf_num_chars = buf.num_graphemes();
        // Don't let the cursor go over the end!
        if buf_num_chars < self.curr_grapheme {
            self.curr_grapheme = buf_num_chars;
        }

        // Can't move past the last character in vi normal mode
        if self.no_eol && self.curr_grapheme != 0 && self.curr_grapheme == buf_num_chars {
            self.curr_grapheme -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::DefaultEditorRules;

    use super::*;

    #[test]
    fn test_clamp_if_pos_is_past_move() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let mut buf = Buffer::from("01234".to_owned());
        cur.move_cursor_to(&buf, 100);
        assert_eq!(5, cur.curr_grapheme);
        cur.reset(&mut buf);
        assert_eq!(0, cur.curr_grapheme);
    }

    #[test]
    fn test_line_widths() {
        let hospital = "\u{1f3e5}";
        let hospital_buf = Buffer::from(hospital.to_owned());
        assert_eq!(hospital_buf.line_widths().collect::<Vec<usize>>(), vec![2]);
        let devanagari_grapheme = "\u{924}\u{947}";
        let devanagari_buf = Buffer::from(devanagari_grapheme.to_owned());
        assert_eq!(
            devanagari_buf.line_widths().collect::<Vec<usize>>(),
            vec![1]
        );
    }

    #[test]
    fn test_clear_exit_for_female_scientist() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        // put some emojis in some strings
        let male_scientist = "\u{1f468}\u{200d}\u{1f52c}".to_owned();
        let female_scientist = "\u{1f469}\u{200d}\u{1f52c}".to_owned();
        let mut decluttered_room = String::new();
        decluttered_room.push_str(&female_scientist);
        decluttered_room.push_str(&male_scientist);

        let mut full_room = String::new();
        full_room.push_str(&male_scientist);
        full_room.push_str(&male_scientist);
        full_room.push_str(&male_scientist);
        full_room.push_str(&decluttered_room);
        let mut buf = Buffer::from(full_room.to_owned());

        cur.move_cursor_right(&buf, 1);
        cur.move_cursor_right(&buf, 1);
        cur.move_cursor_right(&buf, 1);
        cur.delete_until_cursor(&mut buf, 0);
        assert_eq!(decluttered_room.to_owned(), String::from(buf));
    }

    #[test]
    fn test_insert_chars_between_graphemes() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let female_technologist = "\u{1f469}\u{200d}\u{1f4bb}".to_owned();
        let useful_tools = "\u{1f5a5}\u{fe0f}\u{1d4e2}\u{2aff} \u{1d05e} \
        \u{14ab}\u{1fd8}\u{2a4f}\u{2b31}\u{256d}\u{1f5a5}\u{fe0f}";
        let expected = format!(
            "{}{}{}",
            female_technologist, useful_tools, female_technologist
        );
        let mut buf = Buffer::from(format!("{}{}", female_technologist, female_technologist));

        let cs = useful_tools.chars().into_iter().collect::<Vec<char>>();
        cur.move_cursor_right(&buf, 1);
        cur.insert_chars_after_cursor(&mut buf, &cs[..]);
        assert_eq!(expected, String::from(buf));
    }

    #[test]
    fn test_move_cursor() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let female_technologist = "\u{1f469}\u{200d}\u{1f4bb}".to_owned();
        let str = format!("{}{}", female_technologist, female_technologist);
        let mut buf = Buffer::from(str);
        cur.move_cursor_right(&buf, 1);
        cur.delete_before_cursor(&mut buf);
        assert_eq!(female_technologist, String::from(buf));
    }

    #[test]
    fn test_insert_chars_before_cursor() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let female_technologist = "\u{1f469}\u{200d}\u{1f4bb}".to_owned();
        let useful_tools = "\u{1f5a5}\u{fe0f}\u{1d4e2}\u{2aff} \u{1d05e} \
        \u{14ab}\u{1fd8}\u{2a4f}\u{2b31}\u{256d}\u{1f5a5}\u{fe0f}";
        let expected = format!("{}{}", useful_tools, female_technologist);
        let mut buf = Buffer::from(female_technologist);

        let cs = useful_tools.chars().into_iter().collect::<Vec<char>>();
        cur.insert_chars_after_cursor(&mut buf, &cs[..]);
        assert_eq!(expected, String::from(buf));
    }

    #[test]
    fn test_insert_chars_after_cursor() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let female_technologist = "\u{1f469}\u{200d}\u{1f4bb}".to_owned();
        let useful_tools = "\u{1f5a5}\u{fe0f}\u{1d4e2}\u{2aff} \u{1d05e} \
        \u{14ab}\u{1fd8}\u{2a4f}\u{2b31}\u{256d}\u{1f5a5}\u{fe0f}";
        let expected = format!("{}{}", female_technologist, useful_tools);
        let mut buf = Buffer::from(female_technologist);

        let cs = useful_tools.chars().into_iter().collect::<Vec<char>>();
        cur.move_cursor_to_end_of_line(&buf);
        cur.insert_chars_after_cursor(&mut buf, &cs[..]);
        assert_eq!(expected, String::from(buf));
    }

    #[test]
    fn test_clamp_pre_display_adjustments() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let buf = Buffer::from("hello".to_owned());
        cur.curr_grapheme = 8;
        cur.pre_display_adjustment(&buf);
        assert_eq!(5, cur.curr_grapheme)
    }

    #[test]
    fn test_yank_and_paste() {
        let rules = DefaultEditorRules::default();
        let mut cur = Cursor::new_with_divider(&rules);

        let mut buf = Buffer::from("hello hello".to_owned());
        cur.move_cursor_to(&buf, 6);
        cur.yank_all_after_cursor(&mut buf);
        cur.insert_around(&mut buf, true, 1);
        assert_eq!(String::from("hello hhelloello"), String::from(buf));
        assert_eq!(11, cur.curr_grapheme)
    }
}
