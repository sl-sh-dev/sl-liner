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

/// Cursor (as in the terminal's cursor) exposes various functions that modify the
/// Buffer (Buffer for new line or for editing history, that's transparent to Cursor)
/// and maintain the position on the terminal that the actual cursor needs to be
/// drawn.
#[derive(Clone)]
pub struct Cursor<'a> {
    // The location of the cursor. Note that the cursor does not lie on a char, but between chars.
    // So, if `cursor == 0` then the cursor is before the first char,
    // and if `cursor == 1` ten the cursor is after the first char and before the second char.
    char_vec_pos: usize,
    // function to determine how to split words, returns vector of tuples representing index
    // and length of word.
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

    pub fn char_vec_pos(&self) -> usize {
        self.char_vec_pos
    }

    pub fn insert_around(&mut self, buf: &mut Buffer, right: bool, count: usize) {
        let delta = buf.insert_register_around_idx(self.char_vec_pos, count, right);
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

    //TODO issue:
    // because things can come in a character at a time, e.g. technologist
    // emoji, care needs to be taken as to when/how to get a valid
    // char position.
    pub fn delete_until_cursor(&mut self, buf: &mut Buffer, start: usize) {
        let moved = buf.remove(start, self.char_vec_pos);
        self.char_vec_pos -= moved;
    }

    pub fn insert_char_after_cursor(&mut self, buf: &mut Buffer, c: char) {
        let len = buf.insert(self.char_vec_pos, &[c]);
        self.char_vec_pos += len;
    }

    pub fn insert_str_after_cursor(&mut self, buf: &mut Buffer, s: &str) {
        let cs = &s.chars().collect::<Vec<char>>()[..];
        let len = buf.insert(self.char_vec_pos, cs);
        self.char_vec_pos += len;
    }

    pub fn insert_chars_after_cursor(&mut self, buf: &mut Buffer, cs: &[char]) {
        let len = buf.insert(self.char_vec_pos, cs);
        self.char_vec_pos += len;
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
            let str = buf.char_before(cursor_pos);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::get_buffer_words;

    #[test]
    fn test_clamp_if_pos_is_past_move() {
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

        let mut buf = Buffer::from("01234".to_owned());
        cur.move_cursor_to(&buf, 100);
        assert_eq!(5, cur.char_vec_pos);
        cur.reset(&mut buf);
        assert_eq!(0, cur.char_vec_pos);
    }

    #[test]
    fn test_clear_exit_for_female_scientist() {
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

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
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

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
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

        let female_technologist = "\u{1f469}\u{200d}\u{1f4bb}".to_owned();
        let str = format!("{}{}", female_technologist, female_technologist);
        let mut buf = Buffer::from(str);
        cur.move_cursor_right(&buf, 1);
        cur.delete_before_cursor(&mut buf);
        assert_eq!(female_technologist, String::from(buf));
    }

    #[test]
    fn test_insert_chars_before_cursor() {
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

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
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

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
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

        let buf = Buffer::from("hello".to_owned());
        cur.char_vec_pos = 8;
        cur.pre_display_adjustment(&buf);
        assert_eq!(5, cur.char_vec_pos)
    }

    #[test]
    fn test_yank_and_paste() {
        let word_divider_fcn = &Box::new(get_buffer_words);
        let mut cur = Cursor::new(word_divider_fcn);

        let mut buf = Buffer::from("hello".to_owned());
        cur.move_cursor_to(&buf, 0);
        cur.yank_all_after_cursor(&mut buf);
        cur.insert_around(&mut buf, true, 1);
        assert_eq!(String::from("hhelloello"), String::from(buf));
    }
}
