use crate::grapheme_iter::GraphemeIter;
use std::fmt::{self, Write as FmtWrite};
use std::io::{self, Write};
use std::iter::FromIterator;
use unicode_segmentation::UnicodeSegmentation;

/// A modification performed on a `Buffer`. These are used for the purpose of undo/redo.
#[derive(Debug, Clone)]
pub enum Action {
    Insert { start: usize, text: String },
    Remove { start: usize, text: String },
    Noop { start: usize },
    StartGroup,
    EndGroup,
}

impl Action {
    pub fn do_on(&self, buf: &mut Buffer) -> Option<usize> {
        match *self {
            Action::Insert { start, ref text } => {
                buf.insert_raw(start, text);
                Some(start)
            }
            Action::Remove { start, ref text } => {
                let len = text.len();
                buf.remove_raw(start, start + len, false);
                if len > start {
                    Some(0)
                } else {
                    Some(start - len)
                }
            }
            Action::Noop { start } => Some(start),
            Action::StartGroup | Action::EndGroup => None,
        }
    }

    pub fn undo(&self, buf: &mut Buffer) -> Option<usize> {
        match *self {
            Action::Insert { start, ref text } => {
                buf.remove_raw(start, start + text.len(), false);
                Some(start)
            }
            Action::Remove { start, ref text } => {
                buf.insert_raw(start, text);
                Some(start)
            }
            Action::Noop { start } => Some(start),
            Action::StartGroup | Action::EndGroup => None,
        }
    }
}

/// A buffer for text in the line editor.
///
/// It keeps track of each action performed on it for use with undo/redo.
#[derive(Debug, Clone)]
pub struct Buffer {
    data: String,
    actions: Vec<Action>,
    undone_actions: Vec<Action>,
    register: Option<String>,
    curr_num_graphemes: usize,
    grapheme_indices: Vec<usize>,
}

impl PartialEq for Buffer {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}
impl Eq for Buffer {}

impl From<Buffer> for String {
    fn from(buf: Buffer) -> Self {
        let mut to = String::new();
        let mut buf_iter = buf.data.chars().peekable();
        while let Some(ch) = buf_iter.next() {
            if ch == '\\' && buf_iter.peek() == Some(&'\n') {
                // '\\' followed by newline, ignore both.
                buf_iter.next();
                continue;
            }
            to.push(ch);
        }
        to
    }
}

impl From<String> for Buffer {
    fn from(s: String) -> Self {
        s.chars().collect()
    }
}

impl<'a> From<&'a str> for Buffer {
    fn from(s: &'a str) -> Self {
        s.chars().collect()
    }
}

impl fmt::Display for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let chars = self.data.chars();
        for c in chars {
            f.write_char(c)?;
        }
        Ok(())
    }
}

impl FromIterator<char> for Buffer {
    fn from_iter<T: IntoIterator<Item = char>>(t: T) -> Self {
        let str = t.into_iter().collect::<String>();
        let g_idxs = Buffer::string_to_grapheme_indices(&str);
        Buffer {
            data: str,
            actions: Vec::new(),
            undone_actions: Vec::new(),
            register: None,
            curr_num_graphemes: g_idxs.len(),
            grapheme_indices: g_idxs,
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Buffer {
    pub fn new() -> Self {
        Buffer {
            data: String::new(),
            actions: Vec::new(),
            undone_actions: Vec::new(),
            register: None,
            curr_num_graphemes: 0,
            grapheme_indices: Vec::new(),
        }
    }

    pub fn clear_actions(&mut self) {
        self.actions.clear();
        self.undone_actions.clear();
    }

    pub fn start_undo_group(&mut self) {
        self.actions.push(Action::StartGroup);
    }

    pub fn end_undo_group(&mut self) {
        self.actions.push(Action::EndGroup);
    }

    pub fn undo(&mut self) -> Option<usize> {
        use Action::*;

        let mut old_cursor_pos = None;
        let mut group_nest = 0;
        let mut group_count = 0;
        while let Some(act) = self.actions.pop() {
            self.undone_actions.push(act.clone());
            if let Some(pos) = act.undo(self) {
                old_cursor_pos = Some(pos)
            }
            match act {
                EndGroup => {
                    group_nest += 1;
                    group_count = 0;
                }
                StartGroup => group_nest -= 1,
                // count the actions in this group so we can ignore empty groups below
                _ => group_count += 1,
            }

            // if we aren't in a group, and the last group wasn't empty
            if group_nest == 0 && group_count > 0 {
                break;
            }
        }
        old_cursor_pos
    }

    pub fn redo(&mut self) -> Option<usize> {
        use Action::*;

        let mut old_cursor_pos = None;
        let mut group_nest = 0;
        let mut group_count = 0;
        while let Some(act) = self.undone_actions.pop() {
            if let Some(pos) = act.do_on(self) {
                old_cursor_pos = Some(pos)
            }
            self.actions.push(act.clone());
            match act {
                StartGroup => {
                    group_nest += 1;
                    group_count = 0;
                }
                EndGroup => group_nest -= 1,
                // count the actions in this group so we can ignore empty groups below
                _ => group_count += 1,
            }

            // if we aren't in a group, and the last group wasn't empty
            if group_nest == 0 && group_count > 0 {
                break;
            }
        }
        old_cursor_pos
    }

    pub fn revert(&mut self) -> bool {
        if self.actions.is_empty() {
            return false;
        }

        while self.undo().is_some() {}
        true
    }

    fn push_action(&mut self, act: Action) {
        self.actions.push(act);
        self.undone_actions.clear();
    }

    pub fn last_arg(&self) -> Option<String> {
        self.data
            .split_word_bounds()
            .filter(|s| !s.trim().is_empty())
            .last()
            .map(String::from)
    }

    pub fn num_graphemes(&self) -> usize {
        self.curr_num_graphemes
    }

    pub fn num_bytes(&self) -> usize {
        let s: String = self.clone().into();
        s.len()
    }

    fn get_all_graphemes(&self) -> Vec<&str> {
        self.to_graphemes_vec()
    }

    fn get_grapheme(&self, cursor: usize) -> Option<&str> {
        let graphemes = self.to_graphemes_vec();
        graphemes.get(cursor).copied()
    }

    pub fn grapheme_before(&self, cursor: usize) -> Option<&str> {
        self.get_grapheme(cursor - 1)
    }

    pub fn grapheme_after(&self, cursor: usize) -> Option<&str> {
        self.get_grapheme(cursor)
    }

    /// Returns the graphemes removed. Does not register as an action in the undo/redo
    /// buffer or in the buffer's register.
    pub fn remove_unrecorded(&mut self, start: usize, end: usize) {
        self.remove_raw(start, end, false);
    }

    /// Returns the number of graphemes removed.
    pub fn remove(&mut self, start: usize, end: usize) -> usize {
        let orig_len = self.num_graphemes();
        self.remove_raw(start, end, true);
        let new_len = self.num_graphemes();
        orig_len - new_len
    }

    /// Insert contents of register to the right or to the left of the provided start index in the
    /// current buffer
    /// and return length of text inserted.
    pub fn insert_register_around_idx(
        &mut self,
        mut idx: usize,
        count: usize,
        right: bool,
    ) -> usize {
        let mut inserted = 0;
        if let Some(text) = self.register.as_ref() {
            if !text.is_empty() {
                let orig_len = self.num_graphemes();
                if orig_len > idx && right {
                    // insert to right of cursor
                    idx += 1;
                }

                let text = if count > 1 {
                    let mut full_text = String::with_capacity(text.len() * count);
                    for _i in 0..count {
                        full_text.push_str(text);
                    }
                    full_text
                } else {
                    text.to_owned()
                };
                self.insert_action(Action::Insert { start: idx, text });
                let new_len = self.num_graphemes();
                inserted = new_len - orig_len;
            }
        }
        inserted
    }

    pub fn insert_str(&mut self, start: usize, text: &str) -> usize {
        let orig_len = self.num_graphemes();
        let text = text.to_owned();
        let act = Action::Insert { start, text };
        self.insert_action(act);
        let new_len = self.num_graphemes();
        new_len - orig_len
    }

    pub fn insert<'a, I>(&mut self, start: usize, text: I) -> usize
    where
        I: Iterator<Item = &'a char>,
    {
        let text: String = text.collect::<String>();
        self.insert_str(start, &text)
    }

    pub fn insert_action(&mut self, act: Action) {
        act.do_on(self);
        self.push_action(act);
    }

    pub fn append_buffer(&mut self, other: &Buffer) -> usize {
        let start = self.num_graphemes();
        self.insert_str(start, &other.data)
    }

    pub fn copy_buffer(&mut self, other: &Buffer) -> usize {
        self.remove_unrecorded(0, self.curr_num_graphemes);
        self.insert_str(0, &other.data)
    }

    pub fn range_graphemes_all(&self) -> GraphemeIter {
        GraphemeIter::new(&self.data, &self.grapheme_indices)
    }

    pub fn range_graphemes(&self, start: usize, end: usize) -> GraphemeIter {
        if start == 0 && end == self.curr_num_graphemes {
            self.range_graphemes_all()
        } else {
            let start_idx = self.grapheme_indices.get(start);
            let end_idx = self.grapheme_indices.get(end);
            if let (Some(start_idx), Some(end_idx)) = (start_idx, end_idx) {
                GraphemeIter::new_bytes(
                    &self.data.as_bytes()[*start_idx..*end_idx],
                    &self.grapheme_indices[start..end],
                )
            } else {
                GraphemeIter::default()
            }
        }
    }

    pub fn line_widths(&self) -> Vec<usize> {
        self.data
            .split('\n')
            .map(|s| s.graphemes(true).count())
            .collect()
    }

    pub fn lines(&self) -> Vec<String> {
        self.data.split('\n').map(|x| x.to_owned()).collect()
    }

    pub fn graphemes(&self) -> Vec<&str> {
        self.get_all_graphemes()
    }

    pub fn truncate(&mut self, num: usize) {
        self.remove(num, self.num_graphemes());
    }

    pub fn print<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        out.write_all(self.data.as_bytes())
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        // NOTE: not particularly efficient. Could make a proper byte iterator with minimal
        // allocations if performance becomes an issue.
        self.to_string().into_bytes()
    }

    /// Takes other buffer, measures its length and prints this buffer from the point where
    /// the other stopped.
    /// Used to implement autosuggestions.
    pub fn print_rest<W>(&self, out: &mut W, after: usize) -> io::Result<usize>
    where
        W: Write,
    {
        let mut ret = 0;
        if self.curr_num_graphemes != 0 {
            if let Some(&offset) = self.grapheme_indices.get(after) {
                let bytes = &self.data.as_bytes()[offset..];
                out.write_all(bytes)?;
                ret = bytes.len();
            }
        }
        Ok(ret)
    }

    pub fn yank(&mut self, start: usize, end: usize) {
        let graphemes = self.to_graphemes_vec();
        let slice;
        if end >= graphemes.len() {
            slice = &graphemes[start..];
        } else {
            slice = &graphemes[start..end];
        }
        let slice = slice.iter().map(|x| String::from(*x)).collect::<String>();
        self.register = Some(slice);
    }

    fn string_to_graphemes_vec(str: &str) -> Vec<&str> {
        str.graphemes(true).collect::<Vec<&str>>()
    }

    fn to_graphemes_vec(&self) -> Vec<&str> {
        Self::string_to_graphemes_vec(&self.data)
    }

    fn string_to_grapheme_indices(str: &str) -> Vec<usize> {
        str.grapheme_indices(true)
            .map(|o| o.0)
            .collect::<Vec<usize>>()
    }

    fn to_graphemes_indices(&self) -> Vec<usize> {
        Self::string_to_grapheme_indices(&self.data)
    }

    /// done after an insert/remove for two reasons:
    /// 1. the number of graphemes may change
    /// 2. knowing the length of the buffer in graphemes is an important
    /// constant for callers to reference.
    fn recompute_size(&mut self) {
        if self.data.is_empty() {
            self.curr_num_graphemes = 0;
        } else {
            self.grapheme_indices = self.to_graphemes_indices();
            self.curr_num_graphemes = self.grapheme_indices.len();
        }
    }

    /// Push ch onto the end of the buffer.
    pub fn push(&mut self, ch: char) {
        self.data.push(ch);
        self.recompute_size();
    }

    fn remove_raw(&mut self, start: usize, end: usize, save_action: bool) {
        let mut logged_action = false;
        if !self.data.is_empty() && start != end {
            if start == 0 && end == self.num_graphemes() {
                let str = self.data.to_owned();
                self.data.clear();
                if save_action {
                    self.register = Some(str.to_owned());
                    self.push_action(Action::Remove { start, text: str });
                    logged_action = true;
                }
            } else {
                let start_opt = self.grapheme_indices.get(start);
                let len = self.data.len();
                let end_opt= if end >= self.num_graphemes() {
                    Some(&len)
                } else {
                    self.grapheme_indices.get(end)
                };

                if let (Some(start_idx), Some(end_idx)) = (start_opt, end_opt) {
                    let drain = self.data.drain(start_idx..end_idx);
                    if save_action {
                        let str = drain.collect::<String>();
                        self.register = Some(str.to_owned());
                        self.push_action(Action::Remove { start, text: str });
                        logged_action = true;
                    }
                }
            }
        }
        if !logged_action && save_action {
            self.push_action(Action::Noop { start });
        } else {
            self.recompute_size();
        }
    }

    fn insert_raw(&mut self, start: usize, new_graphemes: &str) {
        if start >= self.num_graphemes() {
            let len = self.data.len();
            self.data.insert_str(len, new_graphemes);
        } else {
            let offset = self.grapheme_indices.get(start);
            if let Some(offset) = offset {
                self.data.insert_str(*offset, new_graphemes);
            }
        }
        self.recompute_size();
    }

    /// Check if the other buffer starts with the same content as this one.
    /// Used to implement autosuggestions.
    pub fn starts_with(&self, other: &Buffer) -> bool {
        let other = &other.data;
        let this = &self.data;
        let other_len = other.len();
        let self_len = this.len();
        if !other.is_empty() && self_len != other_len {
            this.starts_with(other)
        } else {
            false
        }
    }

    /// Check if the buffer contains pattern.
    /// Used to implement history search.
    pub fn contains(&self, other: &Buffer) -> bool {
        let other = &other.data;
        if other.is_empty() {
            false
        } else {
            self.data.contains(other)
        }
    }

    /// Return true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns the first grapheme of the buffer or None if empty.
    pub fn first(&self) -> Option<&str> {
        let mut ret = None;
        if !self.data.is_empty() {
            if let Some(str) = self.range_graphemes_all().next() {
                ret = Some(str)
            }
        }
        ret
    }

    /// Returns the last grapheme of the buffer or None if empty.
    pub fn last(&self) -> Option<&str> {
        let mut ret = None;
        if !self.data.is_empty() {
            if let Some(str) = self.range_graphemes_all().rev().next() {
                ret = Some(str)
            }
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_empty() {
        let mut buf = Buffer::new();
        buf.truncate(0);
        assert_eq!(String::from(buf), "");
    }

    #[test]
    fn test_truncate_all() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.truncate(0);
        assert_eq!(String::from(buf), "");
    }

    #[test]
    fn test_truncate_end() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let end = buf.num_graphemes();
        buf.truncate(end);
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_part() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.truncate(3);
        assert_eq!(String::from(buf), "abc");
    }

    #[test]
    fn test_truncate_empty_undo() {
        let mut buf = Buffer::new();
        buf.truncate(0);
        buf.undo();
        assert_eq!(String::from(buf), "");
    }

    #[test]
    fn test_truncate_all_then_undo() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.truncate(0);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_end_then_undo() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let end = buf.num_graphemes();
        buf.truncate(end);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_part_then_undo() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.truncate(3);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_revert_undo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.end_undo_group();
        assert_eq!(String::from(buf.clone()), "defg");
        assert!(buf.revert());
        assert_eq!(String::from(buf), "");
    }

    #[test]
    fn test_clear_undo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.end_undo_group();
        buf.clear_actions();
        buf.revert();
        assert!(buf.undo().is_none());
        assert_eq!(String::from(buf), "defg");
    }

    #[test]
    fn test_undo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.end_undo_group();
        assert!(buf.undo().is_some());
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_redo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.remove(0, 1);
        buf.end_undo_group();
        assert!(buf.undo().is_some());
        assert!(buf.redo().is_some());
        assert_eq!(String::from(buf), "defg");
    }

    #[test]
    fn test_nested_undo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.end_undo_group();
        buf.remove(0, 1);
        buf.end_undo_group();
        assert!(buf.undo().is_some());
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_nested_redo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.start_undo_group();
        buf.remove(0, 1);
        buf.end_undo_group();
        buf.remove(0, 1);
        buf.end_undo_group();
        assert!(buf.undo().is_some());
        assert!(buf.redo().is_some());
        assert_eq!(String::from(buf), "defg");
    }

    #[test]
    fn test_starts_with() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['a', 'b', 'c'].iter());
        assert_eq!(buf.starts_with(&buf2), true);
    }

    #[test]
    fn test_does_not_start_with() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['a', 'b', 'c'].iter());
        assert_eq!(buf.starts_with(&buf2), false);
    }

    #[test]
    fn test_is_not_match2() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['x', 'y', 'z'].iter());
        assert_eq!(buf.starts_with(&buf2), false);
    }

    #[test]
    fn test_partial_eq() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['x', 'y', 'z'].iter());
        assert_eq!(buf.eq(&buf2), false);
        let mut buf3 = Buffer::new();
        buf3.insert(0, ['x', 'y', 'z'].iter());
        assert_eq!(buf2.eq(&buf3), true);
    }

    #[test]
    fn test_buffer_to_string_ignore_newline() {
        let mut buf = Buffer::new();
        buf.insert(0, ['h', 'e', '\\', '\n', 'l', 'l', 'o'].iter());
        assert_eq!("hello".to_owned(), String::from(buf));
    }

    #[test]
    fn test_contains() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['a', 'b', 'c'].iter());
        assert_eq!(buf.contains(&buf2), true);
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['c', 'd', 'e'].iter());
        assert_eq!(buf.contains(&buf2), true);
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['e', 'f', 'g'].iter());
        assert_eq!(buf.contains(&buf2), true);
        let empty_buf = Buffer::default();
        assert_eq!(buf.contains(&empty_buf), false);
    }

    #[test]
    fn test_does_not_contain() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['x', 'b', 'c'].iter());
        assert_eq!(buf.contains(&buf2), false);
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['a', 'b', 'd'].iter());
        assert_eq!(buf.contains(&buf2), false);
    }

    #[test]
    fn test_print() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut out: Vec<u8> = vec![];
        buf.print(&mut out).unwrap();
        assert_eq!(out.len(), 7);
        let mut str = String::new();
        for x in out {
            str.push(x as char);
        }
        assert_eq!(str, String::from("abcdefg"));
    }

    #[test]
    fn test_print_rest() {
        let mut buf = Buffer::new();
        buf.insert(0, ['a', 'b', 'c', 'd', 'e', 'f', 'g'].iter());
        let mut buf2 = Buffer::new();
        buf2.insert(0, ['a', 'b', 'c'].iter());
        let mut out: Vec<u8> = vec![];
        buf.print_rest(&mut out, buf2.data.len()).unwrap();
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn test_append() {
        let orig = String::from("hello string स्ते");
        let mut buf0 = Buffer::from(orig.clone());
        let append = "स्तेa";
        let buf1 = Buffer::from(append);
        buf0.append_buffer(&buf1);
        assert_eq!(Buffer::from(orig + append), buf0);
    }

    #[test]
    fn test_first() {
        let s = "स्ते hello string";
        let buf = Buffer::from(s);
        let first = buf.first();
        assert!(first.is_some());
        let s = "स्";
        assert_eq!(s, first.unwrap());
    }

    #[test]
    fn test_push() {
        let s = "hello string स्ते";
        let mut buf = Buffer::from(s);
        buf.push('a');
        let last_arg = buf.last_arg();
        assert!(last_arg.is_some());
        let s = "स्तेa";
        assert_eq!(String::from(s), last_arg.unwrap());
        let buf = Buffer::from(s);
        let v = buf.as_bytes();
        assert_eq!(
            vec![224, 164, 184, 224, 165, 141, 224, 164, 164, 224, 165, 135, 97],
            v
        );
    }

    #[test]
    fn test_range_chars() {
        let orig = "(“न” “म” “स्” “ते”)";
        let buf = Buffer::from(orig);
        let trim = "(“न” “म” “स्” “";
        let str: String = buf.range_graphemes(0, 14).into();
        assert_eq!(trim, str);
    }

    #[test]
    fn test_range_graphemes_on_empty() {
        let orig = "";
        let buf = Buffer::from(orig);
        let str: String = buf.range_graphemes(5, 14).into();
        assert_eq!(orig, str);
    }
}
