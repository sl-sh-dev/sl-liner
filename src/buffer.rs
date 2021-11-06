use std::fmt::{self, Write as FmtWrite};
use std::io::{self, Write};
use std::iter::FromIterator;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// A modification performed on a `Buffer`. These are used for the purpose of undo/redo.
#[derive(Debug, Clone)]
pub enum Action {
    Insert { start: usize, text: Vec<char> },
    Remove { start: usize, text: Vec<char> },
    StartGroup,
    EndGroup,
}

impl Action {
    pub fn do_on(&self, buf: &mut Buffer) -> Option<usize> {
        match *self {
            Action::Insert { start, ref text } => {
                buf.insert_raw(start, &text[..]);
                Some(start)
            }
            Action::Remove { start, ref text } => {
                let text_len = text.len();
                buf.remove_raw(start, start + text_len);
                if text_len > start {
                    Some(0)
                } else {
                    Some(start - text.len())
                }
            }
            Action::StartGroup | Action::EndGroup => None,
        }
    }

    pub fn undo(&self, buf: &mut Buffer) -> Option<usize> {
        match *self {
            Action::Insert { start, ref text } => {
                buf.remove_raw(start, start + text.len());
                Some(start)
            }
            Action::Remove { start, ref text } => {
                buf.insert_raw(start, &text[..]);
                Some(start)
            }
            Action::StartGroup | Action::EndGroup => None,
        }
    }
}

/// A buffer for text in the line editor.
///
/// It keeps track of each action performed on it for use with undo/redo.
#[derive(Debug, Clone)]
pub struct Buffer {
    data: Vec<char>,
    actions: Vec<Action>,
    undone_actions: Vec<Action>,
    register: Option<Vec<char>>,
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
        let mut buf_iter = buf.data.iter().peekable();
        while let Some(ch) = buf_iter.next() {
            if ch == &'\\' && buf_iter.peek() == Some(&&'\n') {
                // '\\' followed by newline, ignore both.
                buf_iter.next();
                continue;
            }
            to.push(*ch);
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
        for &c in &self.data {
            f.write_char(c)?;
        }
        Ok(())
    }
}

impl FromIterator<char> for Buffer {
    fn from_iter<T: IntoIterator<Item = char>>(t: T) -> Self {
        Buffer {
            data: t.into_iter().collect(),
            actions: Vec::new(),
            undone_actions: Vec::new(),
            register: None,
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
            data: Vec::new(),
            actions: Vec::new(),
            undone_actions: Vec::new(),
            register: None,
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
        //TODO use unicode whitespace functions?
        self.data
            .split(|&c| c == ' ')
            .filter(|s| !s.is_empty())
            .last()
            .map(|chars| chars.iter().collect())
    }

    pub fn num_chars(&self) -> usize {
        self.data.len()
    }

    pub fn num_bytes(&self) -> usize {
        let s: String = self.clone().into();
        s.len()
    }

    fn to_str(c: Option<&char>) -> Option<String> {
        let mut str = None;
        if let Some(c) = c {
            str = Some(c.to_string())
        }
        str
    }

    pub fn char_before(&self, cursor: usize) -> Option<String> {
        let c = self.data.get(cursor - 1);
        Self::to_str(c)
    }

    pub fn char_after(&self, cursor: usize) -> Option<String> {
        let c = self.data.get(cursor);
        Self::to_str(c)
    }

    /// Returns the graphemes removed. Does not register as an action in the undo/redo
    /// buffer or in the buffer's register.
    pub fn remove_silent(&mut self, start: usize, end: usize) -> Vec<String> {
        let end = if end >= self.data.len() {
            self.data.len()
        } else {
            end
        };
        self.remove_raw(start, end)
    }

    /// Returns the number of characters removed.
    pub fn remove(&mut self, start: usize, end: usize) -> usize {
        let removed = self.remove_silent(start, end);
        let mut str = String::from("");
        for x in removed {
            str += &x;
        }
        let chars = str.chars().collect::<Vec<_>>();
        self.push_action(Action::Remove {
            start,
            text: chars.clone(),
        });
        self.register = Some(chars);
        str.width()
    }

    /// Insert contents of register to the right or to the left of the provided start index in the
    /// current buffer
    /// and return length of text inserted.
    pub fn insert_register_around_start(
        &mut self,
        mut start_idx: usize,
        count: usize,
        right: bool,
    ) -> usize {
        let mut inserted = 0;
        if let Some(text) = self.register.as_ref() {
            inserted = text.iter().cloned().collect::<String>().width();
            if inserted > 0 {
                if self.num_chars() > start_idx && right {
                    // insert to right of cursor
                    start_idx += 1;
                }

                let text = if count > 1 {
                    let mut full_text = Vec::with_capacity(text.len() * count);
                    for _i in 0..count {
                        for c in text.iter() {
                            full_text.push(*c);
                        }
                    }
                    inserted *= count;
                    full_text
                } else {
                    text.to_vec()
                };
                self.insert_action(Action::Insert {
                    start: start_idx,
                    text,
                });
            }
        }
        inserted
    }

    pub fn insert(&mut self, start: usize, text: &[char]) -> usize {
        let act = Action::Insert {
            start,
            text: text.into(),
        };
        self.insert_action(act);
        let text: String = text.iter().collect();
        text.width()
    }

    pub fn insert_action(&mut self, act: Action) {
        act.do_on(self);
        self.push_action(act);
    }

    // XXX rename, too confusing
    pub fn insert_from_buffer(&mut self, other: &Buffer) -> usize {
        let start = self.data.len();
        self.insert(start, &other.data[start..])
    }

    pub fn copy_buffer(&mut self, other: &Buffer) -> usize {
        let data_len = self.data.len();
        self.remove(0, data_len);
        self.insert(0, &other.data[0..])
    }

    pub fn range(&self, start: usize, end: usize) -> String {
        self.data[start..end].iter().cloned().collect()
    }

    pub fn range_chars(&self, start: usize, end: usize) -> Vec<String> {
        let s = self.data[start..end].iter().collect::<String>();
        Buffer::graphemes_to_vec(s)
    }

    pub fn width(&self) -> Vec<usize> {
        self.range_width(0, self.num_chars())
    }

    pub fn range_width(&self, start: usize, end: usize) -> Vec<usize> {
        self.range(start, end)
            .split('\n')
            .map(|s| s.width())
            .collect()
    }

    pub fn lines(&self) -> Vec<String> {
        self.data
            .split(|&c| c == '\n')
            .map(|s| s.iter().cloned().collect())
            .collect()
    }

    pub fn graphemes(&self) -> Vec<String> {
        let s = self.data.iter().collect::<String>();
        let graphemes = s.graphemes(true);
        graphemes.map(String::from).collect::<Vec<String>>()
    }

    pub fn truncate(&mut self, num: usize) {
        let end = self.data.len();
        self.remove(num, end);
    }

    pub fn print<W>(&self, out: &mut W) -> io::Result<()>
    where
        W: Write,
    {
        let string: String = self.data.iter().cloned().collect();
        out.write_all(string.as_bytes())
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
        let string: String = self.data.iter().skip(after).cloned().collect();
        out.write_all(string.as_bytes())?;

        Ok(string.len())
    }

    pub fn yank(&mut self, start: usize, end: usize) {
        let slice;
        if end >= self.data.len() {
            slice = &self.data[start..self.data.len()];
        } else {
            slice = &self.data[start..end];
        }
        self.register = Some(slice.to_vec());
    }

    fn graphemes_to_vec(str: String) -> Vec<String> {
        let graphemes = str.graphemes(true);
        graphemes.map(String::from).collect::<Vec<String>>()
    }

    fn remove_raw(&mut self, start: usize, end: usize) -> Vec<String> {
        let cs = self.data.drain(start..end);
        let str = cs.as_ref().iter().collect::<String>();
        Buffer::graphemes_to_vec(str)
    }

    fn insert_raw(&mut self, start: usize, text: &[char]) {
        for (i, &c) in text.iter().enumerate() {
            self.data.insert(start + i, c)
        }
    }

    /// Check if the other buffer starts with the same content as this one.
    /// Used to implement autosuggestions.
    pub fn starts_with(&self, other: &Buffer) -> bool {
        let other_len = other.data.len();
        let self_len = self.data.len();
        if !other.data.is_empty() && self_len != other_len {
            let match_let = self
                .data
                .iter()
                .zip(&other.data)
                .take_while(|&(s, o)| *s == *o)
                .count();
            match_let == other_len
        } else {
            false
        }
    }

    /// Check if the buffer contains pattern.
    /// Used to implement history search.
    pub fn contains(&self, pattern: &Buffer) -> bool {
        let search_term: &[char] = &pattern.data;
        if search_term.is_empty() {
            return false;
        }
        self.data
            .windows(search_term.len())
            .any(|window| window == search_term)
    }

    /// Return true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Returns the first char of the buffer or None if empty.
    pub fn first(&self) -> Option<String> {
        let mut ret = None;
        if !self.data.is_empty() {
            let str = self.data.iter().collect::<String>();
            if let Some(str) = str.graphemes(true).next() {
                ret = Some(String::from(str))
            }
        }
        ret
    }

    /// Returns the last char of the buffer or None if empty.
    pub fn last(&self) -> Option<String> {
        let mut ret = None;
        if !self.data.is_empty() {
            let str = self.data.iter().collect::<String>();
            if let Some(str) = str.graphemes(true).rev().next() {
                ret = Some(String::from(str))
            }
        }
        ret
    }

    /// Push ch onto the endo of the buffer.
    pub fn push(&mut self, ch: char) {
        self.data.push(ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        buf.truncate(0);
        assert_eq!(String::from(buf), "");
    }

    #[test]
    fn test_truncate_end() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let end = buf.num_chars();
        buf.truncate(end);
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_part() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        buf.truncate(0);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_end_then_undo() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let end = buf.num_chars();
        buf.truncate(end);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_truncate_part_then_undo() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        buf.truncate(3);
        buf.undo();
        assert_eq!(String::from(buf), "abcdefg");
    }

    #[test]
    fn test_undo_group() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
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
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['a', 'b', 'c']);
        assert_eq!(buf.starts_with(&buf2), true);
    }

    #[test]
    fn test_does_not_start_with() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['a', 'b', 'c']);
        assert_eq!(buf.starts_with(&buf2), false);
    }

    #[test]
    fn test_is_not_match2() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['x', 'y', 'z']);
        assert_eq!(buf.starts_with(&buf2), false);
    }

    #[test]
    fn test_contains() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['a', 'b', 'c']);
        assert_eq!(buf.contains(&buf2), true);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['c', 'd', 'e']);
        assert_eq!(buf.contains(&buf2), true);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['e', 'f', 'g']);
        assert_eq!(buf.contains(&buf2), true);
    }

    #[test]
    fn test_does_not_contain() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['x', 'b', 'c']);
        assert_eq!(buf.contains(&buf2), false);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['a', 'b', 'd']);
        assert_eq!(buf.contains(&buf2), false);
    }

    #[test]
    fn test_print_rest() {
        let mut buf = Buffer::new();
        buf.insert(0, &['a', 'b', 'c', 'd', 'e', 'f', 'g']);
        let mut buf2 = Buffer::new();
        buf2.insert(0, &['a', 'b', 'c']);
        let mut out: Vec<u8> = vec![];
        buf.print_rest(&mut out, buf2.data.len()).unwrap();
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn test_unicode() {
        use unicode_segmentation::UnicodeSegmentation;

        let s = "नमस्ते";
        let g = s.graphemes(true).collect::<Vec<&str>>();
        println!("cool vec: {:?}.", g);
        let b: &[_] = &["न", "म", "स\u{94d}", "त\u{947}"];
        assert_eq!(g, b);

        println!("len charvec: {}.", "ते".chars().collect::<Vec<char>>().len());
        println!("len charvec: {}.", "न".chars().collect::<Vec<char>>().len());
    }
}
